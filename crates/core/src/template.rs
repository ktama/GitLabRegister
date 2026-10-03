//! タイトルと説明の差し込み項目を実際の値へ置き換える (FR-15、EC-03)。
//!
//! # 記法
//!
//! 要件書 FR-15 は差し込める 7 つの値を定め、記法は設計工程に委ねている。
//! 設計書 版 0.2 も記法を定めていないため、ここで次のとおり定義する
//! (plan.md の逸脱記録 D-1 として報告対象)。
//!
//! - 二重波括弧で項目名を囲む。例: `{{発生日}}`
//! - 項目名は要件書 FR-15 の呼称をそのまま使う
//! - 二重波括弧で囲まれていない波括弧は文字として扱う

use chrono::{Datelike, NaiveDate};

/// 差し込める値 (FR-15 の 7 つ)。
///
/// 列挙型にすることで、項目を追加したときに置換と検証の双方で
/// 取りこぼしがコンパイル時に検出される。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    /// 営業日調整後の発生日。
    OccurrenceDate,
    /// 発生日の年。
    Year,
    /// 発生日の月。
    Month,
    /// 発生日の日。
    Day,
    /// 開始日。
    StartDate,
    /// 期限。
    DueDate,
    /// 定義名。
    ScheduleName,
}

impl Placeholder {
    pub const ALL: [Placeholder; 7] = [
        Placeholder::OccurrenceDate,
        Placeholder::Year,
        Placeholder::Month,
        Placeholder::Day,
        Placeholder::StartDate,
        Placeholder::DueDate,
        Placeholder::ScheduleName,
    ];

    /// 要件書 FR-15 の呼称。
    pub fn name(self) -> &'static str {
        match self {
            Placeholder::OccurrenceDate => "発生日",
            Placeholder::Year => "年",
            Placeholder::Month => "月",
            Placeholder::Day => "日",
            Placeholder::StartDate => "開始日",
            Placeholder::DueDate => "期限",
            Placeholder::ScheduleName => "定義名",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Placeholder::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// 差し込みに使う値。
#[derive(Debug, Clone)]
pub struct RenderContext<'a> {
    pub occurrence_date: NaiveDate,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub schedule_name: &'a str,
}

impl RenderContext<'_> {
    fn value_of(&self, placeholder: Placeholder) -> String {
        match placeholder {
            Placeholder::OccurrenceDate => format_date(self.occurrence_date),
            Placeholder::Year => self.occurrence_date.year().to_string(),
            Placeholder::Month => self.occurrence_date.month().to_string(),
            Placeholder::Day => self.occurrence_date.day().to_string(),
            // 未設定の日付は空文字に置き換える (FR-15)。
            Placeholder::StartDate => self.start_date.map(format_date).unwrap_or_default(),
            Placeholder::DueDate => self.due_date.map(format_date).unwrap_or_default(),
            Placeholder::ScheduleName => self.schedule_name.to_string(),
        }
    }
}

fn format_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// 差し込み項目を値へ置き換える。
///
/// 未知の項目名はそのまま残す。保存時に `unknown_placeholders` で拒否する前提のため、
/// ここでは文字列を失わせない (EC-03 の「入力した文字列をそのまま登録する」と整合)。
pub fn render(text: &str, context: &RenderContext<'_>) -> String {
    rewrite(text, |name| {
        Placeholder::parse(name).map(|p| context.value_of(p))
    })
}

/// 解釈できない差し込み項目名を出現順に返す (FR-15 の保存時の検証)。
pub fn unknown_placeholders(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    rewrite(text, |name| {
        if Placeholder::parse(name).is_none() && !found.iter().any(|n| n == name) {
            found.push(name.to_string());
        }
        None
    });
    found
}

/// `{{...}}` を走査し、`replace` が値を返した箇所だけ置き換える。
fn rewrite(text: &str, mut replace: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after_open = &rest[open + 2..];
        match after_open.find("}}") {
            Some(close) => {
                let name = after_open[..close].trim();
                out.push_str(&rest[..open]);
                match replace(name) {
                    Some(value) => out.push_str(&value),
                    None => out.push_str(&rest[open..open + 2 + close + 2]),
                }
                rest = &after_open[close + 2..];
            }
            // 閉じがない場合は以降をそのまま出力する。
            None => break,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn context<'a>(name: &'a str) -> RenderContext<'a> {
        RenderContext {
            occurrence_date: d(2026, 10, 5),
            start_date: Some(d(2026, 10, 8)),
            due_date: Some(d(2026, 10, 12)),
            schedule_name: name,
        }
    }

    /// FR-15: タイトルに年と月の差し込み項目。発生日 2026-10-05 で 2026 と 10 になる。
    #[test]
    fn template_render_year_and_month() {
        let got = render("{{年}}年{{月}}月分 月次報告", &context("月次報告"));
        assert_eq!(got, "2026年10月分 月次報告");
    }

    /// FR-15: 説明に期限の差し込み項目。期限 2026-10-12 に置き換わる。
    #[test]
    fn template_render_due_date() {
        let got = render("提出期限は {{期限}} です", &context("月次報告"));
        assert_eq!(got, "提出期限は 2026-10-12 です");
    }

    /// FR-15: 期限を設定していない定義で期限の差し込み項目を使うと空文字になる。
    #[test]
    fn template_render_due_date_absent_is_empty() {
        let ctx = RenderContext {
            due_date: None,
            ..context("月次報告")
        };
        let got = render("期限:{{期限}}:終わり", &ctx);
        assert_eq!(got, "期限::終わり");
    }

    /// FR-15: 差し込み項目を含まないタイトルは変化しない。
    #[test]
    fn template_render_without_placeholder() {
        let got = render("週次進捗の報告", &context("週次進捗"));
        assert_eq!(got, "週次進捗の報告");
    }

    /// FR-15: 7 つすべてを置き換えられる。
    #[test]
    fn template_render_all_placeholders() {
        let got = render(
            "{{発生日}}/{{年}}/{{月}}/{{日}}/{{開始日}}/{{期限}}/{{定義名}}",
            &context("棚卸し"),
        );
        assert_eq!(got, "2026-10-05/2026/10/5/2026-10-08/2026-10-12/棚卸し");
    }

    /// FR-15: 7 つに含まれない差し込み項目名を検出する。
    #[test]
    fn template_validate_unknown_placeholder() {
        let got = unknown_placeholders("{{年}}年{{四半期}}期 {{担当}}");
        assert_eq!(got, vec!["四半期".to_string(), "担当".to_string()]);
    }

    #[test]
    fn template_validate_accepts_known_placeholders() {
        assert!(unknown_placeholders("{{発生日}} {{定義名}}").is_empty());
    }

    /// EC-03: 絵文字・サロゲートペア・改行をそのまま保持する。
    #[test]
    fn template_render_preserves_emoji_and_newline() {
        // "𠀋" はサロゲートペアで表される文字。
        let text = "🗓 {{年}}年\n次行 𠀋";
        let got = render(text, &context("確認"));
        assert_eq!(got, "🗓 2026年\n次行 𠀋");
    }

    /// 未知の項目名は置換せずそのまま残す。
    #[test]
    fn template_render_keeps_unknown_placeholder() {
        let got = render("{{四半期}}期 {{年}}", &context("確認"));
        assert_eq!(got, "{{四半期}}期 2026");
    }

    /// 閉じのない波括弧は文字として残す。
    #[test]
    fn template_render_keeps_unclosed_braces() {
        let got = render("{{年}} と {{未閉じ", &context("確認"));
        assert_eq!(got, "2026 と {{未閉じ");
    }
}
