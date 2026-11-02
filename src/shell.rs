//! シェル連携。
//!
//! 実行ファイルは呼び出し元シェルのカレントディレクトリを変えられないため、
//! ラッパー関数を用意する。実行ファイルは `--cwd-file` に指定されたファイルへ
//! 1行目に移動先、2行目に（あれば）移動後に実行するコマンドを書き出し、
//! ラッパー関数がそれを読んで cd・実行する。

use anyhow::{Result, bail};
use clap::ValueEnum;
use std::path::Path;

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
    use anyhow::Context;
    use std::process::{Command, Stdio};
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // explorer は成功しても終了コード 1 を返すことがあるので、終了コードは見ない
    Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("{program} を起動できません: {}", path.display()))?;
    Ok(())
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
        $candidates = @('new', 'ls', 'grep', 'done', 'undone', 'open', 'root', 'import', 'rules', 'config')
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
