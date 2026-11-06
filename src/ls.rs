//! `ls`: 部屋の一覧を tree 形式で表示する（TUI を使わない出力）。

use crate::config::Settings;
use crate::fsinfo::{self, TreeOptions};
use crate::room::{self, Room};
use anyhow::Result;
use chrono::TimeDelta;
use std::io::{IsTerminal, Write};

pub struct LsArgs<'a> {
    pub query: Option<&'a str>,
    pub all: bool,
    /// 直近 N 日より前の、未完了の部屋だけを表示する
    pub old: bool,
    pub days: Option<i64>,
    pub depth: usize,
}

#[derive(Debug, Default)]
pub struct Filter<'a> {
    pub query: Option<&'a str>,
    pub all: bool,
    pub old: bool,
    pub days: Option<i64>,
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("90", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    fn blue(&self, s: &str) -> String {
        self.paint("34", s)
    }
}

/// `ls` とハブで共有する絞り込み。並び順は `room::scan` の新しい順を保つ。
pub fn select_rooms(settings: &Settings, filter: &Filter) -> Result<(Vec<Room>, String)> {
    let days = filter.days.unwrap_or(settings.config.recent_days);
    let cutoff = room::today(settings) - TimeDelta::days(days);
    let filtered = filter.all || filter.old || filter.query.is_some();
    let since = (!filtered).then_some(cutoff);
    let mut rooms = room::scan(&settings.root, since)?;
    let label = if let Some(q) = filter.query {
        let q_lower = q.to_lowercase();
        rooms
            .retain(|r| r.label().to_lowercase().contains(&q_lower) || r.description.to_lowercase().contains(&q_lower));
        format!("「{q}」に一致")
    } else if filter.old {
        rooms.retain(|r| !r.done && r.date < cutoff);
        format!("直近{days}日より前・未完了")
    } else if filter.all {
        "全件".to_string()
    } else {
        rooms.retain(|r| !r.done);
        format!("直近{days}日・進行中")
    };
    Ok((rooms, label))
}

pub fn run(settings: &Settings, args: &LsArgs) -> Result<()> {
    let style = Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let (mut rooms, label) =
        select_rooms(settings, &Filter { query: args.query, all: args.all, old: args.old, days: args.days })?;

    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{}  {}",
        style.bold(&settings.root.display().to_string()),
        style.dim(&format!("({label} {}件)", rooms.len()))
    )?;

    // 案件ごとにまとめる（案件なしが先）
    rooms.sort_by(|a, b| a.project.cmp(&b.project).then_with(|| b.name.cmp(&a.name)));
    let groups = group_by_project(&rooms);
    let n_groups = groups.len();
    for (gi, (project, members)) in groups.into_iter().enumerate() {
        let last_group = gi + 1 == n_groups;
        let base = match project {
            None => String::new(),
            Some(p) => {
                writeln!(out, "{}{}", if last_group { "└── " } else { "├── " }, style.blue(&format!("{p}/")))?;
                if last_group { "    ".to_string() } else { "│   ".to_string() }
            }
        };
        let n = members.len();
        for (i, r) in members.iter().enumerate() {
            // 案件なしのグループの最後の部屋の後に案件グループが続く場合は、まだ最後ではない
            let last = i + 1 == n && (project.is_some() || last_group);
            print_room(&mut out, settings, &style, r, &base, last, args.depth)?;
        }
    }
    Ok(())
}

fn group_by_project(rooms: &[Room]) -> Vec<(Option<&str>, Vec<&Room>)> {
    let mut groups: Vec<(Option<&str>, Vec<&Room>)> = Vec::new();
    for r in rooms {
        let p = r.project.as_deref();
        match groups.last_mut() {
            Some((gp, members)) if *gp == p => members.push(r),
            _ => groups.push((p, vec![r])),
        }
    }
    groups
}

fn print_room(
    out: &mut impl Write,
    settings: &Settings,
    style: &Style,
    r: &Room,
    base: &str,
    last: bool,
    depth: usize,
) -> Result<()> {
    let branch = if last { "└── " } else { "├── " };
    let desc = if r.description.is_empty() { String::new() } else { format!("  {}", style.dim(&r.description)) };
    let icon = if settings.config.icons { format!("{} ", fsinfo::icon_for("", true)) } else { String::new() };
    if r.done {
        writeln!(out, "{base}{branch}{}{desc}", style.dim(&format!("{icon}{}/ [done]", r.name)))?;
    } else {
        writeln!(out, "{base}{branch}{}{desc}", style.cyan(&format!("{icon}{}/", r.name)))?;
    }
    let child_prefix = format!("{base}{}", if last { "    " } else { "│   " });
    let lines = fsinfo::tree(
        &r.path,
        &TreeOptions { depth, max_items: 10, exclude: &settings.config.exclude, hide_room_files: true },
    );
    for l in lines {
        let icon = if settings.config.icons && !l.more {
            format!("{} ", fsinfo::icon_for(&l.name, l.is_dir))
        } else {
            String::new()
        };
        let name = if l.more {
            style.dim(&l.name)
        } else if l.is_dir {
            style.blue(&format!("{icon}{}/", l.name))
        } else {
            format!("{icon}{}", l.name)
        };
        writeln!(out, "{child_prefix}{}{name}", l.prefix)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;

    #[test]
    fn selection_preserves_filters_labels_and_scan_order() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            config: Config { recent_days: 2, ..Config::default() },
            config_path: dir.path().join("config.toml"),
            root: dir.path().join("rooms"),
            state_dir: dir.path().join("state"),
            claude_dir: dir.path().join("claude"),
        };
        let today = room::today(&settings);
        let mut labels = Vec::new();
        for (slug, age, project, description, done) in [
            ("current", 0, None, "新しい Needle", false),
            ("finished", 0, None, "完了", true),
            ("boundary", 2, None, "境界", false),
            ("before", 3, None, "古い未完了", false),
            ("archived", 5, Some("EXAMPLE"), "古い NEEDLE", true),
            ("future", -1, None, "将来の日付", false),
        ] {
            let name = format!("{}-{slug}", today - TimeDelta::days(age));
            let label = project.map_or_else(|| name.clone(), |p| format!("{p}/{name}"));
            let path = settings.root.join(&label);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(room::README), format!("# {description}\n")).unwrap();
            if done {
                fs::write(path.join(room::DONE_MARKER), "").unwrap();
            }
            labels.push(label);
        }
        let cases = [
            (Filter::default(), vec![5, 0, 2], "直近2日・進行中"),
            (Filter { days: Some(0), ..Filter::default() }, vec![5, 0], "直近0日・進行中"),
            (Filter { days: Some(1), ..Filter::default() }, vec![5, 0], "直近1日・進行中"),
            (Filter { old: true, ..Filter::default() }, vec![3], "直近2日より前・未完了"),
            (Filter { old: true, days: Some(0), ..Filter::default() }, vec![2, 3], "直近0日より前・未完了"),
            (Filter { all: true, ..Filter::default() }, vec![5, 1, 0, 2, 3, 4], "全件"),
            (Filter { query: Some("needle"), ..Filter::default() }, vec![0, 4], "「needle」に一致"),
            (Filter { query: Some("BEFORE"), ..Filter::default() }, vec![3], "「BEFORE」に一致"),
            (Filter { query: Some("example/"), ..Filter::default() }, vec![4], "「example/」に一致"),
            (Filter { query: Some("境界"), ..Filter::default() }, vec![2], "「境界」に一致"),
            (Filter { query: Some(""), ..Filter::default() }, vec![5, 1, 0, 2, 3, 4], "「」に一致"),
            (Filter { query: Some("一致なし"), ..Filter::default() }, vec![], "「一致なし」に一致"),
            (Filter { query: Some("needle"), all: true, old: true, days: Some(0) }, vec![0, 4], "「needle」に一致"),
            (Filter { all: true, old: true, ..Filter::default() }, vec![3], "直近2日より前・未完了"),
        ];
        for (filter, indices, expected_label) in cases {
            let (rooms, label) = select_rooms(&settings, &filter).unwrap();
            let actual: Vec<_> = rooms.iter().map(Room::label).collect();
            let expected: Vec<_> = indices.iter().map(|i| labels[*i].clone()).collect();
            assert_eq!(actual, expected, "{filter:?}");
            assert_eq!(label, expected_label, "{filter:?}");
        }
    }

    #[test]
    fn selection_does_not_create_missing_root_or_state() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            config: Config::default(),
            config_path: dir.path().join("config.toml"),
            root: dir.path().join("missing"),
            state_dir: dir.path().join("state"),
            claude_dir: dir.path().join("claude"),
        };
        let (rooms, _) = select_rooms(&settings, &Filter::default()).unwrap();
        assert!(rooms.is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
