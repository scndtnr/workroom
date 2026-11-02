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

pub fn run(settings: &Settings, args: &LsArgs) -> Result<()> {
    let style = Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let days = args.days.unwrap_or(settings.config.recent_days);
    let cutoff = room::today(settings) - TimeDelta::days(days);
    let filtered = args.all || args.old || args.query.is_some();
    let since = (!filtered).then_some(cutoff);
    let mut rooms = room::scan(&settings.root, since)?;
    let label = if let Some(q) = args.query {
        let q_lower = q.to_lowercase();
        rooms
            .retain(|r| r.label().to_lowercase().contains(&q_lower) || r.description.to_lowercase().contains(&q_lower));
        format!("「{q}」に一致")
    } else if args.old {
        rooms.retain(|r| !r.done && r.date < cutoff);
        format!("直近{days}日より前・未完了")
    } else if args.all {
        "全件".to_string()
    } else {
        rooms.retain(|r| !r.done);
        format!("直近{days}日・進行中")
    };

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
