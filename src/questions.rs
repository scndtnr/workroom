//! 確認事項の記録。解析・状態の判定・追記は、CLI とハブで共有する。

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate, SecondsFormat};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const QUESTIONS: &str = "QUESTIONS.jsonl";
pub const ANSWERS: &str = "ANSWERS.jsonl";
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Ask {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommend: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Ask(Ask),
    Done {
        to: String,
        upto: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        chat: Option<String>,
    },
    Withdraw {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        successor: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    pub v: u64,
    pub id: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn new(id: String, by: Option<String>, kind: EventKind) -> Self {
        Self { v: 1, id, at: Local::now().to_rfc3339_opts(SecondsFormat::Secs, false), by, kind }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub ask: Ask,
    pub by: Option<String>,
    pub asked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    Answer,
    NotNeeded,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Answer {
    pub v: u64,
    #[serde(rename = "type")]
    pub record_type: String,
    pub id: String,
    /// 読み取り後はファイル内の順番。食い違った元の値は recorded_n に残す。
    pub n: usize,
    #[serde(default, skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub recorded_n: Option<usize>,
    pub at: String,
    pub kind: AnswerKind,
    pub text: String,
    pub question: Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Unanswered,
    Pending,
    Resolved,
    Withdrawn,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unanswered => "未回答",
            Self::Pending => "回答済み・未反映",
            Self::Resolved => "解決済み",
            Self::Withdrawn => "取り下げ",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Question {
    pub id: String,
    pub ask: Option<Event>,
    pub answers: Vec<Answer>,
    pub done: Vec<Event>,
    pub withdrawals: Vec<Event>,
    pub duplicate_ask: bool,
    pub reflected_upto: usize,
    pub state: State,
}

impl Question {
    fn new(id: String) -> Self {
        Self {
            id,
            ask: None,
            answers: Vec::new(),
            done: Vec::new(),
            withdrawals: Vec::new(),
            duplicate_ask: false,
            reflected_upto: 0,
            state: State::Unanswered,
        }
    }

    pub fn content(&self) -> &Ask {
        if let Some(Event { kind: EventKind::Ask(ask), .. }) = &self.ask { ask } else { &self.answers[0].question.ask }
    }

    pub fn pending_answers(&self) -> &[Answer] {
        &self.answers[self.reflected_upto..]
    }

    fn update_state(&mut self) {
        self.reflected_upto = self
            .done
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Done { upto, .. } => Some(upto),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .min(self.answers.len());
        self.state = if !self.withdrawals.is_empty() {
            State::Withdrawn
        } else if self.answers.len() > self.reflected_upto {
            State::Pending
        } else if !self.done.is_empty() {
            State::Resolved
        } else {
            State::Unanswered
        };
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub file: String,
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "警告: {} の {} 行目: {}", self.file, self.line, self.reason)
    }
}

#[derive(Debug, Default)]
pub struct Questions {
    pub items: BTreeMap<String, Question>,
    pub warnings: Vec<Warning>,
    pub files_exist: bool,
    reserved_ids: BTreeSet<String>,
}

impl Questions {
    fn warn(&mut self, file: &str, line: usize, reason: impl Into<String>) {
        self.warnings.push(Warning { file: file.into(), line, reason: reason.into() });
    }

    pub fn get(&self, id: &str) -> Result<&Question> {
        self.items.get(id).with_context(|| format!("確認事項が見つかりません: {id}"))
    }

    pub fn next_id(&self, date: NaiveDate) -> Result<String> {
        let prefix = format!("Q-{}-", date.format("%Y%m%d"));
        let mut maximum = 0u64;
        for suffix in self.reserved_ids.iter().filter_map(|id| id.strip_prefix(&prefix)) {
            maximum = maximum.max(suffix.parse().context("確認事項の連番が大きすぎます")?);
        }
        let next = maximum.checked_add(1).context("確認事項の連番が大きすぎます")?;
        Ok(format!("{prefix}{next:02}"))
    }
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() >= 13
        && bytes.starts_with(b"Q-")
        && bytes[10] == b'-'
        && bytes[2..10].iter().all(u8::is_ascii_digit)
        && bytes[11..].iter().all(u8::is_ascii_digit)
}

/// 不正な行も行ごとに隔離する。上限の検査はメタデータと実際の読み取りの両方で行う。
fn read_lines(path: &Path) -> Result<Option<Vec<Vec<u8>>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("読めません: {}", path.display())),
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if file.metadata()?.len() > MAX_FILE_BYTES {
        bail!("{name} は 8 MiB を超えているため読めません");
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("{name} は 8 MiB を超えているため読めません");
    }
    Ok(Some(bytes.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect()))
}

pub fn read(dir: &Path) -> Result<Questions> {
    let mut out = Questions::default();
    let id_pattern = regex::Regex::new(r#""id"\s*:\s*"(Q-[0-9]{8}-[0-9]{2,})""#)?;
    let mut done_lines = Vec::new();
    for name in [QUESTIONS, ANSWERS] {
        let Some(lines) = read_lines(&dir.join(name))? else { continue };
        out.files_exist = true;
        for (i, bytes) in lines.iter().enumerate() {
            let line = i + 1;
            let bytes = if i == 0 { bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes) } else { bytes };
            let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            // JSON として壊れていても、読める ID は再利用しない。
            for capture in id_pattern.captures_iter(&String::from_utf8_lossy(bytes)) {
                out.reserved_ids.insert(capture[1].into());
            }
            let value: serde_json::Value = match serde_json::from_slice(bytes) {
                Ok(value) => value,
                Err(_) => {
                    out.warn(name, line, "JSON の形式または UTF-8 が不正です");
                    continue;
                }
            };
            if !value.is_object() {
                out.warn(name, line, "JSON オブジェクトではありません");
                continue;
            }
            if let Some(id) = value["id"].as_str().filter(|id| valid_id(id)) {
                out.reserved_ids.insert(id.into());
            }
            let Some(kind) = value["type"].as_str() else {
                out.warn(name, line, "type がないか文字列ではありません");
                continue;
            };
            if !matches!(kind, "ask" | "done" | "withdraw" | "answer") {
                continue;
            }
            if !value["id"].as_str().is_some_and(valid_id) {
                out.warn(name, line, "id がないか Q-YYYYMMDD-NN の形ではありません");
                continue;
            }
            if (name == QUESTIONS && kind == "answer") || (name == ANSWERS && kind != "answer") {
                out.warn(name, line, "このファイルには書けない種類の記録です");
                continue;
            }
            if value["v"].as_u64().is_some_and(|v| v > 1) {
                out.warn(name, line, "未知のスキーマの版です。読める項目だけを使います");
            }
            if name == QUESTIONS {
                let event: Event = match serde_json::from_value(value) {
                    Ok(event) => event,
                    Err(_) => {
                        out.warn(name, line, "必須の項目がないか、項目の型が不正です");
                        continue;
                    }
                };
                let id = event.id.clone();
                match &event.kind {
                    EventKind::Ask(ask) => {
                        if ask.question.trim().is_empty() || ask.question.contains(['\r', '\n']) {
                            out.warn(name, line, "質問は空でない1行で指定してください");
                            continue;
                        }
                        let item = out.items.entry(id.clone()).or_insert_with(|| Question::new(id));
                        if item.ask.is_some() {
                            item.duplicate_ask = true;
                            out.warn(name, line, "ask が重複しています。最初の起票を使います");
                        } else {
                            item.ask = Some(event);
                        }
                    }
                    EventKind::Done { upto, .. } => {
                        if let Some(item) = out.items.get_mut(&id).filter(|q| q.ask.is_some()) {
                            done_lines.push((id, line, *upto));
                            item.done.push(event);
                        } else {
                            out.warn(name, line, "ask のない done を無視しました");
                        }
                    }
                    EventKind::Withdraw { .. } => {
                        if let Some(item) = out.items.get_mut(&id).filter(|q| q.ask.is_some()) {
                            item.withdrawals.push(event);
                        } else {
                            out.warn(name, line, "ask のない withdraw を無視しました");
                        }
                    }
                }
            } else {
                let mut answer: Answer = match serde_json::from_value(value) {
                    Ok(answer) => answer,
                    Err(_) => {
                        out.warn(name, line, "必須の項目がないか、項目の型が不正です");
                        continue;
                    }
                };
                let id = answer.id.clone();
                let n = out.items.get(&id).map_or(1, |q| q.answers.len() + 1);
                if answer.n != n {
                    out.warn(name, line, "n が順番と一致しません。ファイル内の順番を使います");
                    answer.recorded_n = Some(answer.n);
                    answer.n = n;
                }
                out.items.entry(id.clone()).or_insert_with(|| Question::new(id)).answers.push(answer);
            }
        }
    }
    for (id, line, upto) in done_lines {
        if upto > out.items[&id].answers.len() {
            out.warn(QUESTIONS, line, "upto が回答の件数を超えています。回答の件数までとして扱います");
        }
    }
    for item in out.items.values_mut() {
        item.update_state();
    }
    Ok(out)
}

/// 部屋の中の排他的な作成で追記を直列化する（File::lock は MSRV 1.88 では使えない）。
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_with(path, Duration::from_secs(5), Duration::from_millis(50), Duration::from_secs(30))
    }

    fn acquire_with(path: &Path, timeout: Duration, interval: Duration, stale: Duration) -> Result<Self> {
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let guard = Self { path: path.into() };
                    writeln!(file, "pid={} at={}", std::process::id(), Local::now().to_rfc3339())?;
                    return Ok(guard);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified())
                        && SystemTime::now().duration_since(modified).unwrap_or_default() > stale
                    {
                        match std::fs::remove_file(path) {
                            Ok(()) => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(e) => return Err(e).context("古いロックを削除できません"),
                        }
                    }
                    if start.elapsed() >= timeout {
                        bail!("確認事項のロックを取得できません: {}", path.display());
                    }
                    std::thread::sleep(interval.min(timeout.saturating_sub(start.elapsed())));
                }
                Err(e) => return Err(e).context("確認事項のロックを作れません"),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// ロックを持ったまま読み直して検査・採番し、1回の write_all でイベントを追記する。
/// None は冪等な操作で、ファイルを作成・変更しない。
pub fn append<T>(
    dir: &Path,
    build: impl FnOnce(&Questions) -> Result<(Option<Event>, T)>,
) -> Result<(T, Vec<Warning>)> {
    let _guard = Lock::acquire(&dir.join(format!(".{QUESTIONS}.lock")))?;
    let current = read(dir)?;
    let (event, result) = build(&current).map_err(|error| {
        if current.warnings.is_empty() {
            error
        } else {
            let warnings = current.warnings.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
            error.context(warnings)
        }
    })?;
    if let Some(event) = event {
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        append_bytes(&dir.join(QUESTIONS), bytes)?;
    }
    Ok((result, current.warnings))
}

/// 呼び出し元が対象のロックを保持し、全件の検査とシリアライズを終えてから呼ぶ。
fn append_bytes(path: &Path, mut bytes: Vec<u8>) -> Result<()> {
    let mut last = [0u8];
    match File::open(path) {
        Ok(mut file) => {
            use std::io::{Seek, SeekFrom};
            if file.metadata()?.len() > 0 {
                file.seek(SeekFrom::End(-1))?;
                file.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    bytes.insert(0, b'\n');
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("追記先を読めません"),
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("追記できません: {}", path.display()))?
        .write_all(&bytes)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct AnswerRequest {
    #[serde(default)]
    pub id: String,
    pub kind: String,
    pub text: String,
}

#[derive(Debug)]
pub enum AnswerError {
    Invalid { id: Option<String>, message: &'static str },
    TooLarge { id: String },
    Withdrawn { id: String, reason: String, successor: Option<String> },
    Storage(anyhow::Error),
}

impl From<anyhow::Error> for AnswerError {
    fn from(error: anyhow::Error) -> Self {
        Self::Storage(error)
    }
}

/// 回答だけを直列化する。質問の書き手とは別のロックで、追記直前の両ファイルを読む。
pub fn answer(dir: &Path, answers: Vec<AnswerRequest>) -> std::result::Result<Vec<String>, AnswerError> {
    let _guard = Lock::acquire(&dir.join(format!(".{ANSWERS}.lock")))?;
    // GET と同様に、部屋の外を指す JSONL を読み書きしない。
    let canonical = dir.canonicalize().context("部屋を確認できません")?;
    for name in [QUESTIONS, ANSWERS] {
        let path = dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            let target = path.canonicalize().context("確認事項のファイルを確認できません")?;
            if !target.starts_with(&canonical) || !target.is_file() {
                return Err(anyhow::anyhow!("部屋の外や通常のファイルでない記録は読み書きできません: {name}").into());
            }
        }
    }
    let current = read(dir)?;
    if answers.is_empty() {
        return Err(AnswerError::Invalid { id: None, message: "回答を1件以上指定してください" });
    }
    let mut sent = BTreeSet::new();
    let mut records = Vec::with_capacity(answers.len());
    let at = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    for request in answers {
        let invalid = |message| AnswerError::Invalid { id: Some(request.id.clone()), message };
        let item = current
            .items
            .get(&request.id)
            .filter(|q| q.ask.is_some())
            .ok_or_else(|| invalid("確認事項が見つかりません"))?;
        if item.duplicate_ask {
            return Err(invalid("ask が重複しているため回答できません"));
        }
        if let Some(Event { kind: EventKind::Withdraw { reason, successor }, .. }) = item.withdrawals.last() {
            return Err(AnswerError::Withdrawn {
                id: request.id,
                reason: reason.clone(),
                successor: successor.clone(),
            });
        }
        let kind = match request.kind.as_str() {
            "answer" => AnswerKind::Answer,
            "not_needed" => AnswerKind::NotNeeded,
            _ => return Err(invalid("kind は answer か not_needed を指定してください")),
        };
        if !sent.insert(request.id.clone()) {
            return Err(invalid("同じ ID が1回の送信に2つあります"));
        }
        if request.text.len() > 64 * 1024 {
            return Err(AnswerError::TooLarge { id: request.id });
        }
        let normalized = request.text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<_> = normalized.split('\n').collect();
        let start = lines.iter().position(|line| !line.trim().is_empty()).unwrap_or(lines.len());
        let end = lines.iter().rposition(|line| !line.trim().is_empty()).map_or(start, |i| i + 1);
        let text = lines[start..end].join("\n");
        if kind == AnswerKind::Answer && text.is_empty() {
            return Err(invalid("回答の本文を入力してください"));
        }
        let event = item.ask.as_ref().unwrap();
        records.push(Answer {
            v: 1,
            record_type: "answer".into(),
            id: request.id,
            n: item.answers.len() + 1,
            recorded_n: None,
            at: at.clone(),
            kind,
            text,
            question: Snapshot { ask: item.content().clone(), by: event.by.clone(), asked_at: Some(event.at.clone()) },
        });
    }
    let mut bytes = Vec::new();
    for record in &records {
        serde_json::to_writer(&mut bytes, record).context("回答を JSON に変換できません")?;
        bytes.push(b'\n');
    }
    append_bytes(&dir.join(ANSWERS), bytes)?;
    Ok(records.into_iter().map(|record| record.id).collect())
}

pub fn ask(dir: &Path, date: NaiveDate, mut ask: Ask, by: Option<String>) -> Result<((String, usize), Vec<Warning>)> {
    if ask.question.contains(['\r', '\n']) {
        bail!("質問は改行を含めず1行で指定してください");
    }
    ask.question = ask.question.trim().into();
    if ask.question.is_empty() || ask.question.chars().count() > 1000 {
        bail!("質問は1文字以上1000文字以内で指定してください");
    }
    if ask.recommend.is_some_and(|n| n == 0 || n > ask.choices.len()) {
        bail!("--recommend は選択肢の番号（1 から）で指定してください");
    }
    append(dir, |current| {
        if let Some(id) = &ask.origin {
            current.get(id)?;
        }
        let id = current.next_id(date)?;
        let unanswered = current.items.values().filter(|q| q.state == State::Unanswered).count() + 1;
        Ok((Some(Event::new(id.clone(), by, EventKind::Ask(ask))), (id, unanswered)))
    })
}

pub struct DoneRequest {
    pub id: String,
    pub upto: Option<usize>,
    pub to: Option<String>,
    pub chat: Option<String>,
    pub by: Option<String>,
}

pub struct DoneResult {
    pub to: Option<String>,
    pub answers: Vec<Answer>,
}

pub fn done(dir: &Path, request: DoneRequest) -> Result<(DoneResult, Vec<Warning>)> {
    append(dir, |current| {
        let item = current.get(&request.id)?;
        if item.state == State::Withdrawn {
            bail!("取り下げ済みです: {}", request.id);
        }
        let chat = request.chat.as_deref().map(str::trim);
        if chat == Some("") {
            bail!("--chat には会話で受けた回答の要旨を指定してください");
        }
        let upto = request.upto.unwrap_or(item.answers.len()).min(item.answers.len());
        if chat.is_none() && (!item.done.is_empty() || request.upto.is_some()) && upto <= item.reflected_upto {
            // 未回答の --upto 0 は、回答のないまま閉じないため下のエラーに進める。
            if !item.done.is_empty() || !item.answers.is_empty() {
                return Ok((None, DoneResult { to: None, answers: Vec::new() }));
            }
        }
        if upto <= item.reflected_upto && chat.is_none() {
            bail!(
                "反映する回答がありません。会話で回答を受けたなら --chat を付けてください。質問を無効にするなら withdraw を使ってください"
            );
        }
        if item.ask.is_none() {
            bail!("起票のない確認事項は解決を記録できません: {}", request.id);
        }
        let newly_reflected = &item.answers[item.reflected_upto.min(upto)..upto];
        let to = match request.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(to) => to.to_string(),
            None if chat.is_none()
                && !newly_reflected.is_empty()
                && newly_reflected.iter().all(|a| a.kind == AnswerKind::NotNeeded) =>
            {
                "回答不要".into()
            }
            None => bail!("反映先を --to で指定してください"),
        };
        let answers = if request.upto.is_none() { newly_reflected.to_vec() } else { Vec::new() };
        let event = Event::new(
            request.id,
            request.by,
            EventKind::Done { to: to.clone(), upto, chat: chat.map(str::to_string) },
        );
        Ok((Some(event), DoneResult { to: Some(to), answers }))
    })
}

pub struct WithdrawResult {
    pub changed: bool,
    pub done: Vec<Event>,
    pub pending_count: usize,
}

pub fn withdraw(
    dir: &Path,
    id: &str,
    reason: String,
    successor: Option<String>,
    by: Option<String>,
) -> Result<(WithdrawResult, Vec<Warning>)> {
    if reason.trim().is_empty() {
        bail!("取り下げる理由を指定してください");
    }
    append(dir, |current| {
        let item = current.get(id)?;
        if item.state == State::Withdrawn {
            return Ok((None, WithdrawResult { changed: false, done: Vec::new(), pending_count: 0 }));
        }
        if item.ask.is_none() {
            bail!("起票のない確認事項は取り下げを記録できません: {id}");
        }
        if let Some(id) = &successor {
            current.get(id)?;
        }
        let event = Event::new(id.into(), by, EventKind::Withdraw { reason, successor });
        Ok((
            Some(event),
            WithdrawResult { changed: true, done: item.done.clone(), pending_count: item.pending_answers().len() },
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const ID: &str = "Q-20261106-01";

    fn event(kind: &str) -> Value {
        let mut event = json!({"v":1,"type":kind,"id":ID,"at":"2026-11-06T10:00:00+09:00"});
        match kind {
            "ask" => event["question"] = json!("出力形式はどうしますか"),
            "done" => {
                event["to"] = json!("README");
                event["upto"] = json!(0);
            }
            "withdraw" => event["reason"] = json!("前提が変わった"),
            _ => unreachable!(),
        }
        event
    }

    fn answer(n: usize, kind: &str) -> Value {
        json!({"v":1,"type":"answer","id":ID,"n":n,"at":"2000-01-01T00:00:00Z",
            "kind":kind,"text":"日本語の回答\n次の行","question":{"question":"写しの質問"}})
    }

    fn write_values(dir: &Path, name: &str, values: &[Value]) {
        let text = values.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn states_follow_counts_and_order_instead_of_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let mut done = event("done");
        done["upto"] = json!(1);
        let cases = [
            (vec![event("ask")], vec![], State::Unanswered, 0),
            (vec![event("ask")], vec![answer(1, "answer")], State::Pending, 0),
            (vec![event("ask"), done.clone()], vec![answer(1, "answer")], State::Resolved, 1),
            (vec![event("ask"), done.clone()], vec![answer(1, "answer"), answer(2, "answer")], State::Pending, 1),
            (vec![event("ask")], vec![answer(1, "not_needed")], State::Pending, 0),
            (vec![event("ask"), event("done")], vec![], State::Resolved, 0),
            (
                vec![event("ask"), done.clone(), event("withdraw")],
                vec![answer(1, "answer"), answer(2, "answer")],
                State::Withdrawn,
                1,
            ),
            (vec![event("ask"), done.clone()], vec![], State::Resolved, 0),
        ];
        for (events, answers, state, upto) in cases {
            write_values(dir.path(), QUESTIONS, &events);
            write_values(dir.path(), ANSWERS, &answers);
            let data = read(dir.path()).unwrap();
            assert_eq!(data.items[ID].state, state);
            assert_eq!(data.items[ID].reflected_upto, upto);
        }
        // 最大の upto を使う。ファイル内の最後の done でも日時でもない。
        write_values(dir.path(), QUESTIONS, &[event("ask"), done, event("done")]);
        write_values(dir.path(), ANSWERS, &[answer(1, "answer")]);
        assert_eq!(read(dir.path()).unwrap().items[ID].reflected_upto, 1);
    }

    #[test]
    fn accepts_bom_crlf_blank_lines_and_a_valid_unterminated_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(QUESTIONS), format!("\u{feff}{}\r\n\r\n{}", event("ask"), event("done")))
            .unwrap();
        let data = read(dir.path()).unwrap();
        assert!(data.warnings.is_empty());
        assert_eq!(data.items[ID].state, State::Resolved);
    }

    #[test]
    fn broken_lines_are_isolated_and_forward_compatible_records_survive() {
        let dir = tempfile::tempdir().unwrap();
        let mut future = event("ask");
        future["v"] = json!(2);
        future["future_field"] = json!({"nested":true});
        let bad_id = json!({"type":"ask","id":"wrong"});
        let missing = json!({"v":1,"type":"ask","id":ID,"at":"now"});
        let text = format!("broken\n[]\n{{}}\n{bad_id}\n{missing}\n{{\"type\":\"future\"}}\n{future}\n");
        std::fs::write(dir.path().join(QUESTIONS), text).unwrap();
        let data = read(dir.path()).unwrap();
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.warnings.len(), 6);
        assert_eq!(data.warnings.iter().map(|w| w.line).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 7]);
        assert!(data.warnings.iter().all(|w| w.file == QUESTIONS));
        std::fs::write(dir.path().join(QUESTIONS), b"\xff\n").unwrap();
        assert_eq!(read(dir.path()).unwrap().warnings.len(), 1);
    }

    #[test]
    fn first_ask_wins_and_orphan_events_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut duplicate = event("ask");
        duplicate["question"] = json!("重複の質問");
        write_values(dir.path(), QUESTIONS, &[event("done"), event("withdraw"), event("ask"), duplicate]);
        let data = read(dir.path()).unwrap();
        assert_eq!(data.warnings.len(), 3);
        assert!(data.items[ID].duplicate_ask);
        assert!(data.items[ID].done.is_empty());
        assert!(data.items[ID].withdrawals.is_empty());
        assert_eq!(data.items[ID].content().question, "出力形式はどうしますか");
    }

    #[test]
    fn answer_snapshot_and_effective_numbers_survive_missing_questions() {
        let dir = tempfile::tempdir().unwrap();
        write_values(dir.path(), ANSWERS, &[answer(8, "answer"), answer(1, "not_needed")]);
        let data = read(dir.path()).unwrap();
        assert_eq!(data.items[ID].content().question, "写しの質問");
        assert_eq!(data.items[ID].state, State::Pending);
        assert_eq!(data.items[ID].answers[0].n, 1);
        assert_eq!(data.items[ID].answers[0].recorded_n, Some(8));
        assert_eq!(data.items[ID].answers[1].n, 2);
        assert_eq!(data.warnings.len(), 2);
    }

    #[test]
    fn warns_when_upto_exceeds_answers_and_uses_last_withdrawal() {
        let dir = tempfile::tempdir().unwrap();
        let mut done = event("done");
        done["upto"] = json!(99);
        let mut withdrawn = event("withdraw");
        withdrawn["reason"] = json!("最後の理由");
        write_values(dir.path(), QUESTIONS, &[event("ask"), done, event("withdraw"), withdrawn]);
        let data = read(dir.path()).unwrap();
        assert_eq!(data.items[ID].reflected_upto, 0);
        assert_eq!(data.items[ID].withdrawals.len(), 2);
        assert_eq!(data.warnings.len(), 1);
        assert_eq!(data.warnings[0].line, 2);
    }

    #[test]
    fn numbering_reserves_ids_from_broken_lines_and_separates_dates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(QUESTIONS), "{\"id\":\"Q-20261106-99\",broken\n{\"id\":\"Q-20261105-999\"}\n")
            .unwrap();
        let data = read(dir.path()).unwrap();
        assert_eq!(data.next_id(NaiveDate::from_ymd_opt(2026, 11, 6).unwrap()).unwrap(), "Q-20261106-100");
        assert_eq!(data.next_id(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()).unwrap(), "Q-20261107-01");
        assert!(!valid_id("Q-20261106-1"));
        assert!(!valid_id("Q-日本語-01"));
    }

    #[test]
    fn append_preserves_every_existing_byte_and_separates_a_partial_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(QUESTIONS);
        let original = format!("{}\n{{\"id\":\"Q-20261106-02\",", event("ask"));
        std::fs::write(&path, &original).unwrap();
        let ask = Ask { question: " 新しい質問 ".into(), ..Ask::default() };
        let ((id, _), warnings) = ask_question(dir.path(), ask);
        assert_eq!(id, "Q-20261106-03");
        assert_eq!(warnings.len(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(original.as_bytes()));
        let added = std::str::from_utf8(&bytes[original.len()..]).unwrap();
        assert!(added.starts_with('\n'));
        assert!(added.contains("新しい質問"));
        assert!(!added.contains("\\u"));
        assert!(!added.contains('\r'));
        assert_eq!(read(dir.path()).unwrap().items.len(), 2);
        assert!(!dir.path().join(".QUESTIONS.jsonl.lock").exists());
    }

    fn ask_question(dir: &Path, value: Ask) -> ((String, usize), Vec<Warning>) {
        ask(dir, NaiveDate::from_ymd_opt(2026, 11, 6).unwrap(), value, None).unwrap()
    }

    #[test]
    fn rejects_files_larger_than_eight_mib_without_reading_them() {
        let dir = tempfile::tempdir().unwrap();
        for name in [QUESTIONS, ANSWERS] {
            let path = dir.path().join(name);
            File::create(&path).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
            assert!(read(dir.path()).unwrap_err().to_string().contains("8 MiB"));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn lock_waits_times_out_and_is_removed_on_drop_and_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".QUESTIONS.jsonl.lock");
        let lock = Lock::acquire(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("pid="));
        let start = Instant::now();
        assert!(
            Lock::acquire_with(&path, Duration::from_millis(30), Duration::from_millis(5), Duration::from_secs(30))
                .is_err()
        );
        assert!(start.elapsed() >= Duration::from_millis(30));
        drop(lock);
        assert!(!path.exists());
        let result: Result<((), _)> = append(dir.path(), |_| bail!("検査で失敗"));
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn stale_lock_is_reclaimed_and_live_lock_waits_for_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".QUESTIONS.jsonl.lock");
        let file = File::create(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(31))).unwrap();
        drop(file);
        let lock = Lock::acquire(&path).unwrap();
        let waiter_path = path.clone();
        let waiter = std::thread::spawn(move || Lock::acquire(&waiter_path).unwrap());
        std::thread::sleep(Duration::from_millis(70));
        drop(lock);
        drop(waiter.join().unwrap());
        assert!(!path.exists());
    }

    fn request(id: &str, text: &str) -> AnswerRequest {
        AnswerRequest { id: id.into(), kind: "answer".into(), text: text.into() }
    }

    #[test]
    fn answer_append_preserves_partial_lines_counts_records_and_copies_the_ask() {
        let dir = tempfile::tempdir().unwrap();
        let mut ask = event("ask");
        ask["background"] = json!("質問の背景\n2行目");
        ask["origin"] = json!("Q-20261105-01");
        write_values(dir.path(), QUESTIONS, &[ask]);
        let original = format!("{}\n壊れた末尾", answer(9, "answer"));
        std::fs::write(dir.path().join(ANSWERS), &original).unwrap();
        assert_eq!(super::answer(dir.path(), vec![request(ID, &"a".repeat(65536))]).unwrap(), [ID]);
        let bytes = std::fs::read(dir.path().join(ANSWERS)).unwrap();
        assert!(bytes.starts_with(original.as_bytes()));
        let added = std::str::from_utf8(&bytes[original.len()..]).unwrap();
        assert!(added.starts_with('\n'));
        assert!(added.ends_with('\n'));
        let record: Value = serde_json::from_str(added.trim()).unwrap();
        assert_eq!(record["n"], 2);
        assert_eq!(record["text"].as_str().unwrap().len(), 65536);
        assert_eq!(record["question"]["background"], "質問の背景\n2行目");
        assert_eq!(record["question"]["origin"], "Q-20261105-01");
        assert_eq!(read(dir.path()).unwrap().items[ID].answers.len(), 2);
        assert!(!dir.path().join(".ANSWERS.jsonl.lock").exists());
    }

    #[test]
    fn concurrent_answers_get_distinct_contiguous_numbers() {
        let dir = tempfile::tempdir().unwrap();
        write_values(dir.path(), QUESTIONS, &[event("ask")]);
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let path = dir.path().to_path_buf();
                std::thread::spawn(move || super::answer(&path, vec![request(ID, "並行した回答")]).unwrap())
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let data = read(dir.path()).unwrap();
        assert!(data.warnings.is_empty());
        assert_eq!(data.items[ID].answers.iter().map(|a| a.n).collect::<Vec<_>>(), (1..=8).collect::<Vec<_>>());
        assert!(!dir.path().join(".ANSWERS.jsonl.lock").exists());
    }

    #[test]
    fn answers_reread_questions_after_acquiring_the_lock_and_reject_orphan_ids() {
        let dir = tempfile::tempdir().unwrap();
        write_values(dir.path(), QUESTIONS, &[event("ask")]);
        let lock = Lock::acquire(&dir.path().join(".ANSWERS.jsonl.lock")).unwrap();
        let path = dir.path().to_path_buf();
        let worker = std::thread::spawn(move || super::answer(&path, vec![request(ID, "送信中の回答")]));
        write_values(dir.path(), QUESTIONS, &[event("ask"), event("withdraw")]);
        drop(lock);
        assert!(matches!(worker.join().unwrap(), Err(AnswerError::Withdrawn { .. })));
        assert!(!dir.path().join(ANSWERS).exists());
        assert!(!dir.path().join(".ANSWERS.jsonl.lock").exists());
        std::fs::remove_file(dir.path().join(QUESTIONS)).unwrap();
        write_values(dir.path(), ANSWERS, &[answer(1, "answer")]);
        let before = std::fs::read(dir.path().join(ANSWERS)).unwrap();
        assert!(matches!(
            super::answer(dir.path(), vec![request(ID, "起票のない項目")]),
            Err(AnswerError::Invalid { .. })
        ));
        assert_eq!(std::fs::read(dir.path().join(ANSWERS)).unwrap(), before);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn answers_do_not_follow_jsonl_links_outside_the_room() {
        let dir = tempfile::tempdir().unwrap();
        let room = dir.path().join("room");
        std::fs::create_dir(&room).unwrap();
        let outside = dir.path().join("outside.jsonl");
        std::fs::write(&outside, "部屋の外の内容\n").unwrap();
        #[cfg(unix)]
        let symlink = |from: &Path, to: &Path| std::os::unix::fs::symlink(from, to);
        #[cfg(windows)]
        let symlink = |from: &Path, to: &Path| std::os::windows::fs::symlink_file(from, to);
        for name in [QUESTIONS, ANSWERS] {
            let link = room.join(name);
            if let Err(error) = symlink(&outside, &link) {
                #[cfg(windows)]
                if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                    eprintln!("シンボリックリンクの作成権限がないためスキップ: {error}");
                    return;
                }
                panic!("シンボリックリンクを作れません: {error}");
            }
            assert!(matches!(super::answer(&room, vec![request(ID, "回答")]), Err(AnswerError::Storage(_))));
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "部屋の外の内容\n");
            assert!(!room.join(".ANSWERS.jsonl.lock").exists());
            std::fs::remove_file(link).unwrap();
        }
    }
}
