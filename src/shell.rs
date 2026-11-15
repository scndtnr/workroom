//! シェル連携。
//!
//! 実行ファイルは呼び出し元シェルのカレントディレクトリを変えられないため、
//! ラッパー関数を用意する。実行ファイルは `--cwd-file` に指定されたファイルへ
//! 1行目に移動先、2行目に（あれば）移動後に実行するコマンドを書き出し、
//! ラッパー関数がそれを読んで cd・実行する。

use anyhow::{Result, bail};
use clap::ValueEnum;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Pwsh,
    Nu,
    Bash,
    Zsh,
}

/// 移動後に起動できるコマンド（任意のコマンドは受け付けない）
pub const AGENT_COMMANDS: [&str; 2] = ["claude", "codex"];

pub fn write_cwd_file(cwd_file: Option<&Path>, dest: &Path, then: Option<&str>) -> Result<()> {
    match cwd_file {
        Some(file) => {
            let mut text = format!("{}\n", dest.display());
            if let Some(cmd) = then {
                if !AGENT_COMMANDS.contains(&cmd) {
                    bail!("起動できないコマンドです: {cmd}");
                }
                text.push_str(cmd);
                text.push('\n');
            }
            std::fs::write(file, text)?;
        }
        None => {
            // ラッパーを通さずに実行された場合は、パスを表示するだけ
            println!("{}", dest.display());
            if then.is_some() {
                eprintln!("（シェル連携が未設定のため、コマンドは起動しません。`workroom init --help` を参照）");
            }
        }
    }
    Ok(())
}

/// ファイラーで開く。TUI の画面を崩さないよう、起動したプログラムの出力は捨てる
pub fn open_in_filer(path: &Path) -> Result<()> {
    open_with_os(path.as_os_str())
}

/// OS に渡すパスから Windows のドライブ・UNC の verbatim prefix を外す。
/// 文字列の変換だけを行い、通常のパスと非 Unicode のパスはそのまま保つ。
pub fn strip_verbatim_for_open(path: &OsStr) -> OsString {
    let Some(text) = path.to_str() else { return path.to_owned() };
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}").into();
    }
    if let Some(drive) = text.strip_prefix(r"\\?\")
        && drive.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && drive.as_bytes().get(1..3) == Some(b":\\")
    {
        return drive.into();
    }
    path.to_owned()
}

/// ファイル・フォルダ専用。Windows ではカンマも含めて1つのパスとして引用する。
pub fn open_path_with_os(path: &OsStr) -> Result<()> {
    use anyhow::Context;
    let mut command = os_open_path_command(path)?;
    command.spawn().with_context(|| {
        format!("{} を起動できません: {}", command.get_program().to_string_lossy(), path.to_string_lossy())
    })?;
    Ok(())
}

#[cfg(windows)]
fn explorer_path_argument(path: &OsStr) -> Result<OsString> {
    let bytes = path.as_encoded_bytes();
    if bytes.is_empty() || bytes.contains(&b'"') || bytes.last() == Some(&b'\\') {
        bail!("空のパス、引用符を含むパス、末尾がバックスラッシュのパスは開けません");
    }
    let mut quoted = OsString::from("\"");
    quoted.push(path);
    quoted.push("\"");
    Ok(quoted)
}

fn os_open_path_command(path: &OsStr) -> Result<Command> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer");
        command.raw_arg(explorer_path_argument(path)?).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        Ok(command)
    }
    #[cfg(not(windows))]
    {
        Ok(os_open_command(path))
    }
}

/// パスや URL を OS の既定のアプリで開く。起動したプログラムの出力は捨てる。
pub fn open_with_os(target: &OsStr) -> Result<()> {
    use anyhow::Context;
    let mut command = os_open_command(target);
    // explorer は成功しても終了コード 1 を返すことがあるので、終了コードは見ない
    command.spawn().with_context(|| {
        format!("{} を起動できません: {}", command.get_program().to_string_lossy(), target.to_string_lossy())
    })?;
    Ok(())
}

fn os_open_command(target: &OsStr) -> Command {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    command.arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

pub fn validate_cmd_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !name.starts_with('-');
    if !ok {
        bail!("--cmd には英数字・ハイフン・アンダースコアのみ使えます: {name}");
    }
    Ok(())
}

/// ラッパー関数のスクリプト。
///
/// ASCII だけで書くこと。PowerShell は外部コマンドの出力を `[Console]::OutputEncoding` で読むので、
/// 日本語版 Windows の既定（shift_jis）では UTF-8 の日本語が化け、`Invoke-Expression` が構文エラーになる。
pub fn init_script(shell: Shell, cmd: &str) -> String {
    let template = match shell {
        Shell::Pwsh => PWSH,
        Shell::Nu => NU,
        Shell::Bash | Shell::Zsh => POSIX,
    };
    template.replace("__CMD__", cmd)
}

const PWSH: &str = r#"# workroom shell integration (PowerShell)
function __CMD__ {
    # Resolve the executable so that a function named "workroom" does not call itself
    $__wr_exe = (Get-Command workroom -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $__wr_tmp = [System.IO.Path]::GetTempFileName()
    try {
        & $__wr_exe --cwd-file $__wr_tmp @args
        $__wr_lines = @(Get-Content -LiteralPath $__wr_tmp -Encoding utf8)
        if ($__wr_lines.Count -ge 1 -and $__wr_lines[0]) {
            Set-Location -LiteralPath $__wr_lines[0]
            if ($__wr_lines.Count -ge 2 -and $__wr_lines[1] -in @('claude', 'codex')) {
                & $__wr_lines[1]
            }
        }
    } finally {
        Remove-Item -LiteralPath $__wr_tmp -ErrorAction SilentlyContinue
    }
}

Register-ArgumentCompleter -Native -CommandName __CMD__ -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.ToString() })
    if ($wordToComplete) { $words = @($words | Select-Object -SkipLast 1) }
    if ($words.Count -eq 0) {
        $candidates = @('new', 'ls', 'hub', 'grep', 'done', 'undone', 'open', 'root', 'import', 'rules', 'q', 'config')
    } elseif ($words[0] -in @('done', 'undone', 'open') -and $words.Count -eq 1) {
        $__wr_exe = (Get-Command workroom -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
        if (-not $__wr_exe) { return }
        $__wr_encoding = [Console]::OutputEncoding
        try {
            [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
            $candidates = @(& $__wr_exe names 2>$null)
        } finally {
            [Console]::OutputEncoding = $__wr_encoding
        }
    } else {
        return
    }
    $candidates | Where-Object { $_ -like "*$wordToComplete*" } |
        ForEach-Object { [System.Management.Automation.CompletionResult]::new($_) }
}
"#;

const NU: &str = r#"# workroom shell integration (nushell)
def --env --wrapped __CMD__ [...rest] {
    let tmp = (mktemp -t workroom.XXXXXX)
    # A non-zero exit code raises an error; remove the temp file before returning
    try { ^workroom --cwd-file $tmp ...$rest } catch { rm -f $tmp; return }
    let lines = (open --raw $tmp | decode utf-8 | lines)
    rm -f $tmp
    if ($lines | length) >= 1 and ($lines | first | is-not-empty) {
        cd ($lines | first)
        if ($lines | length) >= 2 and ($lines | get 1) in [claude codex] {
            run-external ($lines | get 1)
        }
    }
}
"#;

const POSIX: &str = r#"# workroom shell integration (bash / zsh)
__CMD__() {
    local __wr_tmp __wr_dest __wr_then __wr_rc
    __wr_tmp="$(mktemp)" || return 1
    command workroom --cwd-file "$__wr_tmp" "$@"
    __wr_rc=$?
    { IFS= read -r __wr_dest; IFS= read -r __wr_then; } < "$__wr_tmp"
    rm -f "$__wr_tmp"
    if [ -n "$__wr_dest" ] && [ -d "$__wr_dest" ]; then
        cd -- "$__wr_dest" || return 1
        case "$__wr_then" in
            claude|codex) command "$__wr_then" ;;
        esac
    fi
    return $__wr_rc
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn path_opener_always_quotes_explorer_arguments_without_launching_it() {
        for path in [r"C:\work\evil.exe,x.md", r"C:\work\報告 書.md", r"\\server\share\a,b.md", r"C:\work\a&b.txt"] {
            let quoted = explorer_path_argument(OsStr::new(path)).unwrap();
            assert_eq!(quoted, OsString::from(format!("\"{path}\"")));
            let command = os_open_path_command(OsStr::new(path)).unwrap();
            assert_eq!(command.get_program(), OsStr::new("explorer"));
            assert_eq!(command.get_args().collect::<Vec<_>>(), [quoted.as_os_str()]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn path_opener_rejects_quotes_empty_paths_and_trailing_backslashes() {
        for path in ["", "C:\\work\\a\"b.md", "C:\\", "C:\\work\\"] {
            assert!(os_open_path_command(OsStr::new(path)).is_err(), "{path}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn path_opener_preserves_literal_arguments_on_other_platforms() {
        let path = OsStr::new("/work/evil.exe,x.md");
        let command = os_open_path_command(path).unwrap();
        assert_eq!(command.get_program(), os_open_command(path).get_program());
        assert_eq!(command.get_args().collect::<Vec<_>>(), [path]);
    }

    #[test]
    fn strips_verbatim_drive_and_unc_prefixes_without_os_calls() {
        for (input, expected) in [
            (r"\\?\C:\work\報告.md", r"C:\work\報告.md"),
            (r"\\?\UNC\server\share\報告.md", r"\\server\share\報告.md"),
            (r"\\.\C:\work\報告.md", r"\\.\C:\work\報告.md"),
            (r"\\?\Volume{example}\報告.md", r"\\?\Volume{example}\報告.md"),
            (r"C:\work\報告.md", r"C:\work\報告.md"),
            ("/work/報告.md", "/work/報告.md"),
        ] {
            assert_eq!(strip_verbatim_for_open(OsStr::new(input)), OsStr::new(expected));
        }
    }

    #[test]
    fn os_opener_passes_paths_and_urls_as_one_literal_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("作業 フォルダ").join("a&b.txt");
        let url = OsStr::new("http://127.0.0.1:47100/?room=EXAMPLE%2F2026-11-03&theme=dark#file");
        for target in [path.as_os_str(), url] {
            let command = os_open_command(target);
            let program = if cfg!(windows) {
                "explorer"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            assert_eq!(command.get_program(), OsStr::new(program));
            assert_eq!(command.get_args().collect::<Vec<_>>(), vec![target]);
        }
    }

    #[cfg(unix)]
    #[test]
    fn os_opener_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;
        let target = OsStr::from_bytes(b"/tmp/workroom-\xff");
        assert_eq!(os_open_command(target).get_args().collect::<Vec<_>>(), vec![target]);
    }

    #[test]
    fn cmd_name_is_substituted() {
        for shell in [Shell::Pwsh, Shell::Nu, Shell::Bash] {
            let s = init_script(shell, "wk");
            assert!(!s.contains("__CMD__"));
            assert!(s.contains("wk"));
            assert!(s.is_ascii(), "シェル連携のスクリプトは ASCII のみ");
        }
    }

    #[test]
    fn cmd_name_validation() {
        assert!(validate_cmd_name("wk").is_ok());
        assert!(validate_cmd_name("w_r-2").is_ok());
        assert!(validate_cmd_name("").is_err());
        assert!(validate_cmd_name("a;rm").is_err());
        assert!(validate_cmd_name("-x").is_err());
    }

    #[test]
    fn cwd_file_rejects_unknown_command() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("cwd");
        assert!(write_cwd_file(Some(&f), dir.path(), Some("rm")).is_err());
        write_cwd_file(Some(&f), dir.path(), Some("claude")).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text, format!("{}\nclaude\n", dir.path().display()));
    }
}
