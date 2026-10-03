# 実装計画: GitLabRegister コア層

## 根拠文書

- 要件: `docs/requirements.md` 版 0.6 (FR 23 / NFR 18 / EC 36)
- 設計: `docs/design.md` 版 0.2 (ADR-01〜ADR-08 すべて承認済み)

## 今回の範囲

設計書 ADR-02 のコア層 (副作用なし) とその単体テストに限る。
サービス層・インフラ層・UI 層は本計画の対象外とし、別計画で扱う。

対象コンポーネント (設計書 第 4 節):
RecurrenceEngine / BusinessDayCalendar / DateOffsetCalculator / TemplateRenderer / HistoryIndex / InputValidator

## 制約 (実装中ずっと守るもの)

- コア層は通信もファイルも時計も参照しない (ADR-02、ADR-06)。現在日付は引数で受け取る
- 日付は日付のみを持つ型で扱う。時刻とタイムゾーンを計算に持ち込まない (ADR-06、NFR-14)
- 文字数はコードポイント数で数える。サロゲートペアで表される文字を 1 文字とする (EC-02)
- 繰り返しパターンと営業日調整は列挙型とし、網羅検査を効かせる (ADR-01 の決め手)
- 依頼にない機能を追加しない。気づいた問題は「発見事項」に記録して報告する

## 変更前の状態 (Phase 0 の記録)

- 既存コードなし。`docs/` のみ。テスト対象が存在しないため「変更前のテスト成否」は該当なし
- Rust 1.96.0 / cargo 1.96.0 / Node 22.16.0 / npm 11.4.2 / WebView2 154.0.4258.48
- Tauri CLI は未導入 (本計画の範囲外。UI 層の着手時に必要)
- git リポジトリは初期化済み、ブランチ main、コミットなし、リモートなし
- コミットは依頼者の指示があるまで行わない。検証の証拠は本ファイルの実行記録に残す

## 追加する依存 (実装の自由範囲として決定、報告で明示)

| 依存 | 用途 | 選定理由 |
|---|---|---|
| chrono (default-features = false) | 日付のみの型と暦計算 | 標準ライブラリに日付型がない。既定機能を切ることで時計へのアクセスを型レベルで排除し、ADR-06 の「現在日付は引数で受け取る」を構造的に強制する |
| serde (derive) | エンティティの直列化 | ADR-03 のテキスト永続化でインフラ層が使う。型をコア層に一本化し重複定義を避ける |

## 受け入れ基準 → テスト対応表

| 基準 (要件 ID) | テスト | 状態 |
| --- | --- | --- |
| FR-04 毎日 | `recurrence::daily_covers_every_day_in_range` | 通過 |
| FR-04 毎営業日 | `recurrence::daily_every_business_day_excludes_weekend_and_holiday` | 通過 |
| FR-04 N 日ごと (起算日から) | `recurrence::daily_every_n_days_counts_from_anchor`, `recurrence::daily_every_n_days_ignores_dates_before_anchor` | 通過 |
| FR-04 毎週 月木 1 週ごと | `recurrence::weekly_every_week_mon_thu` | 通過 |
| FR-04 隔週 起算日 2026-10-05 火曜 (2 か月で 4 件) | `recurrence::weekly_biweekly_tuesday_two_months` | 通過 |
| FR-04 隔週 展開期間に依存しない | `recurrence::weekly_biweekly_independent_of_range` | 通過 |
| FR-04 隔週 火曜と金曜 | `recurrence::weekly_biweekly_tuesday_and_friday` | 通過 |
| FR-04 隔週 起算日より前は 0 件 | `recurrence::weekly_biweekly_before_anchor_is_empty` | 通過 |
| FR-04 毎月 25 日を 12 か月 | `recurrence::monthly_day25_twelve_months` | 通過 |
| FR-04 毎月 末日 | `recurrence::monthly_last_day` | 通過 |
| FR-04 毎月 第 2 火曜 | `recurrence::monthly_second_tuesday` | 通過 |
| FR-04 毎月 31 日 末日に繰り上げ (EC-20) | `recurrence::monthly_day31_clamp_to_last_day` | 通過 |
| FR-04 毎月 31 日 その月は発生させない (EC-20) | `recurrence::monthly_day31_skip_month` | 通過 |
| FR-04 毎年 | `recurrence::yearly_same_month_and_day_each_year` | 通過 |
| FR-04 N 日ごと / N 週ごとの範囲外を拒否 | `validator::validator_interval_out_of_range` | 通過 |
| FR-04 2 週以上で起算日未指定を拒否 | `validator::validator_weekly_interval_requires_anchor` | 通過 |
| FR-04 曜日未選択を拒否 | `validator::validator_weekly_requires_weekday` | 通過 |
| FR-04 起算日のない隔週は算出 0 件 (逸脱 D-4) | `recurrence::weekly_interval_without_anchor_yields_nothing` | 通過 |
| FR-05 毎月 1 日 後ろ倒し | `business_day::business_day_adjust_next_business_day` | 通過 |
| FR-05 毎月 1 日 前倒し (期間外へ出る) | `business_day::business_day_adjust_previous_business_day_outside_range`, `expand::expand_keeps_candidate_adjusted_outside_range` | 通過 |
| FR-05 毎月 1 日 その回を発生させない | `business_day::business_day_adjust_skip_occurrence` | 通過 |
| FR-05 毎月 1 日 調整しない | `business_day::business_day_adjust_none` | 通過 |
| FR-05 土日が同じ翌営業日に重なり 1 件 | `expand::expand_adjust_collision_is_deduplicated` | 通過 |
| FR-05 連続する非営業日をまたぐ探索 | `business_day::business_day_adjust_skips_consecutive_non_business_days` | 通過 |
| FR-06 開始日 0 / 期限 7 | `offset::offset_start0_due7` | 通過 |
| FR-06 開始日 3 / 期限 7 | `offset::offset_start3_due7` | 通過 |
| FR-06 開始日のみ指定 | `offset::offset_start_only` | 通過 |
| FR-06 期限 0 は発生日と同日 | `offset::offset_due0_is_same_day` | 通過 |
| FR-06 両方未指定 | `offset::offset_none` | 通過 |
| FR-06 月・年をまたぐ加算 | `offset::offset_crosses_month_and_year` | 通過 |
| FR-07 複数定義が 1 つの一覧になる | `expand::expand_merges_multiple_schedules` | 通過 |
| FR-07 期間の逆転を拒否 | `expand::expand_rejects_reversed_range` | 通過 |
| FR-07 候補 0 件では登録できない | `expand::expand_cannot_register_when_empty` | 通過 |
| FR-11 登録済みの発生日を除外 | `history::history_excludes_registered`, `expand::expand_excludes_registered_dates` | 通過 |
| FR-11 別定義の同じ発生日は残る | `history::history_keeps_other_schedule` | 通過 |
| FR-09 登録成功を索引に反映 | `history::history_record_excludes_on_next_expansion` | 通過 |
| FR-14 無効化した定義は展開しない | `expand::expand_skips_disabled_schedule` | 通過 |
| FR-15 年と月の差し込み | `template::template_render_year_and_month` | 通過 |
| FR-15 期限の差し込み | `template::template_render_due_date` | 通過 |
| FR-15 7 項目すべての差し込み | `template::template_render_all_placeholders` | 通過 |
| FR-15 未知の差し込み項目名を拒否 | `template::template_validate_unknown_placeholder`, `validator::validator_rejects_unknown_placeholder` | 通過 |
| FR-15 期限未設定なら空文字 | `template::template_render_due_date_absent_is_empty` | 通過 |
| FR-15 差し込みなしは変化しない | `template::template_render_without_placeholder` | 通過 |
| FR-15 展開時に差し込みが適用される | `expand::expand_renders_placeholders` | 通過 |
| FR-21 有効期間 2026-04〜09 で 6 件 | `recurrence::active_period_limits_occurrences` | 通過 |
| FR-21 有効期間なしは制限されない | `recurrence::active_period_absent_no_limit` | 通過 |
| FR-21 開始日のみ指定 | `recurrence::active_period_start_only` | 通過 |
| FR-21 有効期間の逆転を拒否 | `validator::validator_active_period_reversed` | 通過 |
| FR-23 履歴削除で再び候補になる | `history::history_forget_makes_date_candidate_again` | 通過 |
| NFR-01 定義 50 件・92 日を 3 秒以内 | `performance::nfr01_expand_50_schedules_over_92_days_within_3s` | 通過 (1.59 ms) |
| NFR-04 定義 200 件・履歴 10 万件 | `performance::nfr04_expand_200_schedules_within_3s`, `performance::nfr04_history_index_build_100k_within_1s` | 通過 (4.58 ms / 10.80 ms) |
| EC-01 名称・タイトルが空 | `validator::validator_empty_title_and_name` | 通過 |
| EC-02 タイトル 255 文字は可 / 256 文字は拒否 | `validator::validator_title_256_chars` | 通過 |
| EC-02 説明の上限 | `validator::validator_description_limit` | 通過 |
| EC-02 サロゲートペアを 1 文字と数える | `validator::validator_counts_codepoints` | 通過 |
| EC-03 絵文字・サロゲートペア・改行をそのまま保持 | `template::template_render_preserves_emoji_and_newline` | 通過 |
| EC-05 候補が上限 500 件を超えると登録不可 | `expand::expand_warns_too_many_candidates` | 通過 |
| EC-06 オフセット範囲外を拒否 | `validator::validator_offset_out_of_range` | 通過 |
| EC-21 第 5 金曜のない月 | `recurrence::monthly_fifth_friday_missing_month` | 通過 |
| EC-22 閏年でない年の 2 月 29 日 | `recurrence::yearly_feb29_non_leap_year` | 通過 |
| EC-23 休日未登録の年を判別し警告する | `business_day::calendar_reports_year_without_holidays`, `expand::expand_warns_year_without_holidays`, `expand::expand_no_holiday_warning_without_adjustment` | 通過 |
| EC-24 基準日を引数で受け取る (プリセット) | `expand::expand_uses_injected_today` | 通過 |
| EC-26 履歴が定義名を保持する | `history::history_entry_keeps_schedule_name` | 通過 |
| EC-30 定義 ID が不変で履歴が対応する | `history::history_key_survives_pattern_change` | 通過 |
| EC-31 起算日が未来 | `recurrence::weekly_biweekly_future_anchor` | 通過 |
| EC-36 開始日 > 期限 を拒否 / 等しければ可 | `validator::validator_start_after_due`, `validator::validator_start_equals_due_is_allowed` | 通過 |

## ステップ

- [x] S1: cargo ワークスペースと core クレートの骨格、ビルド成果物を除外する `.gitignore` [対応: ADR-02] 検証: `cargo build` が成功
- [x] S2: ドメイン型の定義 (日付範囲・繰り返しパターン・調整種別・定義・Issue テンプレート・履歴) [対応: 設計書 5.1] 検証: `cargo build` が成功
- [x] S3: BusinessDayCalendar の営業日判定と調整 [対応: FR-05、EC-23] 検証: `cargo test business_day` で該当テストが通る
- [x] S4: RecurrenceEngine 毎日・毎営業日・N 日ごと [対応: FR-04] 検証: `cargo test recurrence::daily` が通る
- [x] S5: RecurrenceEngine 毎週・N 週ごと (起算日) [対応: FR-04、EC-31] 検証: `cargo test recurrence::weekly` が通る
- [x] S6: RecurrenceEngine 毎月日付・毎月第 N 曜日・毎年 [対応: FR-04、EC-20〜EC-22] 検証: `cargo test recurrence::monthly recurrence::yearly` が通る
- [x] S7: 有効期間フィルタと調整後の重複束ね、展開の入口関数 [対応: FR-21、FR-05、FR-07、EC-24] 検証: `cargo test expand` が通る
- [x] S8: DateOffsetCalculator [対応: FR-06、EC-36] 検証: `cargo test offset` が通る
- [x] S9: TemplateRenderer [対応: FR-15、EC-03] 検証: `cargo test template` が通る
- [x] S10: InputValidator [対応: EC-01、EC-02、EC-06、EC-36、FR-04] 検証: `cargo test validator` が通る
- [x] S11: HistoryIndex [対応: FR-11、EC-30] 検証: `cargo test history` が通る
- [x] S12: スイート全体の実行と受け入れ基準の突合 [対応: 全件] 検証: `cargo test` が全件成功し、対応表の全行が通ったテストを持つ

## 実行記録

- Phase 0: `ls -a` → `.git` と `docs/` のみ。既存コードなし
- Phase 0: `cargo search chrono` → crates.io に到達可能 (chrono 0.4.45)
- S1-S2: `cargo test -p gitlabregister-core` → 失敗。`Datelike` トレイトが未インポートで `weekday()` が非公開扱い (E0624)。import を追加して解消
- S3: `cargo test` → 8 passed; 0 failed (BusinessDayCalendar)
- S4-S7(算出): `cargo test` → 30 passed; 0 failed (RecurrenceEngine を追加)
- S8-S9: `cargo test` → 46 passed; 0 failed (DateOffsetCalculator、TemplateRenderer を追加)
- S10-S11: `cargo test` → 66 passed; 0 failed (InputValidator、HistoryIndex を追加)
- S7(展開の入口): `cargo test` → 78 passed; 0 failed (expand を追加)
- S12: `cargo test` → 78 passed; 0 failed。`cargo build` の警告 0 件
- S12: 性能計測を追加した直後、履歴生成の剰余が相関して索引が一意キー 2,000 件に潰れていることを検出。生成方法を商と剰余に分けて 100,000 件の一意キーになるよう修正
- S12: 修正後に履歴が展開期間を覆い尽くし登録候補が 0 件になったため、履歴の日付範囲を 2025-06-01 から 500 日分に変更し、除外と候補の双方が計測に含まれるようにした
- S12: `cargo test --release --test performance -- --nocapture` → 4 passed; 0 failed
  - NFR-01 (定義 50 件・92 日・履歴 10 万件): **1.59 ms** (要件 3 秒以内)。登録候補 1,067 件 / 除外 191 件
  - NFR-04 (定義 200 件): **4.58 ms** (要件 3 秒以内)。登録候補 4,200 件、上限超過で登録不可 (EC-05)
  - 設計書 R-4 (履歴 10 万件の索引構築): **10.80 ms**、一意キー 100,000 件
  - 営業日判定 100,000 回: **538 µs**
- 完了検証: `cargo test` → 78 passed (単体) + 4 passed (性能) + 0 failed。Phase 0 時点でテスト対象が存在しなかったため「新たに壊れたもの」は該当なし
- 完了検証: 対応表のテスト名を実際の関数名と突合して修正 (当初の計画名と実装名が食い違っていた)。全 67 行が通ったテストを持つ

## 逸脱記録

### D-1: 差し込み項目の記法が設計書で未定義 [分類: 設計への影響あり / 要承認]

要件書 FR-15 は「差し込み項目の記法は設計工程で定める」と委ねているが、設計書 版 0.2 の
TemplateRenderer の責務にも記法の定義がない。実装できないため、次のとおり定めた。

- 二重波括弧で項目名を囲む。例: `{{発生日}}`
- 項目名は要件書 FR-15 の呼称をそのまま使う (発生日 / 年 / 月 / 日 / 開始日 / 期限 / 定義名)
- 二重波括弧で囲まれていない波括弧は文字として扱う
- 閉じのない `{{` 以降はそのまま出力する

承認されれば設計書 第 4 節 TemplateRenderer に追記する。

### D-2: 年・月・日の差し込みはゼロ埋めしない [分類: 実装の自由範囲]

FR-15 の受け入れ基準は発生日 2026-10-05 で「2026 と 10 に置き換わる」ことのみを定め、
1 桁の月日の表記を定めていない。ゼロ埋めせず `10`、`5` と出す。
日付そのものの差し込み (発生日・開始日・期限) は `2026-10-05` 形式でゼロ埋めする。

### D-3: 営業日の探索上限を 366 日とした [分類: 実装の自由範囲]

FR-05 は前倒し・後ろ倒しの探索に期間外の日付も含めると定めるが上限を定めていない。
休日カレンダーの誤登録で探索が止まらなくなることを防ぐ安全弁として 366 日を上限とした。
連続する非営業日が 366 日続くことは暦上ありえないため、正常な運用に影響しない。

### D-4: 起算日のない隔週パターンは算出 0 件とした [分類: 実装の自由範囲]

FR-04 は「N が 2 以上の場合は起算日を指定する」と定め、InputValidator が保存時に拒否する。
ただし FR-18 の取り込みで起算日のないデータが入りうるため、算出側でも 0 件を返す二重の防御とした。

## 発見事項 (触っていない既存の問題)

- 設計書 第 10 節のリスク R-4 (履歴 10 万件の索引構築が NFR-01 の 3 秒に収まるか) は
  本計画の性能計測で解消した (索引構築 10.80 ms、展開と合わせて 12.4 ms 程度)。
  設計書 第 11 節の未解決事項 2「履歴索引を永続化するか」は不要と判断できる材料が揃った。
  設計書の更新は本計画の範囲外のため、報告で提案する
- 設計書 第 10 節のリスク R-2 (同時実行数 2 の妥当性) と R-3、R-5、R-6 は
  通信層・UI 層に属するため本計画では検証できない。別計画で扱う


---

# 実装計画 2: GitLabRegister インフラ層 (永続化・ログ・秘密保持)

## 今回の範囲

設計書 第 4 節のインフラ層のうち、外部通信を伴わない 3 つに限る。

- FileStore (ADR-03、ADR-04、EC-28)
- Logger (FR-17、NFR-07、NFR-10)
- SecretHolder (NFR-05)

GitLabClient は別計画で扱う。理由は、設計書 第 10 節のリスク R-3 (イテレーション設定のエラー分類) と
R-5 (ライセンスティアの判定方法) が実インスタンスなしでは確認できず、
先に作っても証拠が残らないためである。エラー分類と再試行の規則は局所試験で検証できるので、
その計画では応答を模した試験サーバーを立てる。

## 制約

- 保存先は実行する利用者の Windows ユーザープロファイル配下に限る (NFR-09)
- 全体置換と原子的置換は一時ファイルへ書き切ってから名前を置き換える (設計書 5.3)
- 履歴の追記は 1 件ごとにディスクへ確定させる (ADR-04)
- 永続データは人が読み、テキストエディタで編集できる形式にする (FR-18)
- ログに PAT を書かない。SecretHolder から Logger への経路を作らない (NFR-07)

## 追加する依存 (実装の自由範囲として決定)

| 依存 | 用途 | 選定理由 |
|---|---|---|
| serde_json | 構造化テキストの読み書き | ADR-03 の「人が読めるテキスト」を整形出力で満たす。TOML は定義の入れ子配列が読みにくく、YAML は依存が重い。履歴は 1 行 1 件の JSON Lines にして追記と相性を取る |
| tempfile (開発依存のみ) | 試験用の一時ディレクトリ | 本体には含めない |

## 受け入れ基準 → テスト対応表 2

| 基準 (要件 ID) | テスト | 状態 |
| --- | --- | --- |
| FR-01 接続先 URL を保存し PAT は保存しない | `store::document_settings_round_trip_without_secrets` | 通過 |
| FR-02 登録先プロジェクトの読み書き | `store::document_projects_round_trip` | 通過 |
| FR-03 定義の読み書き | `store::document_schedules_round_trip` | 通過 |
| FR-13 休日の読み書き | `store::holiday_io_export_then_import` | 通過 |
| FR-18 定義ファイルが人に読める | `atomic::schema_saved_file_is_human_readable`, `store::document_schedules_file_is_human_readable` | 通過 |
| FR-18 10 件を書き出して 0 件の端末で取り込む | `store::transfer_export_then_import_on_empty_store` | 通過 |
| FR-18 テキストエディタで編集して定義を追加できる | `store::transfer_edited_file_can_add_schedule` | 通過 |
| FR-18 同一名称 + 上書き | `store::transfer_import_overwrite_replaces_existing` | 通過 |
| FR-18 同一名称 + 取り込まない | `store::transfer_import_skip_keeps_existing_and_reports_names` | 通過 |
| FR-18 形式不正は 1 件も反映しない | `store::transfer_import_malformed_file_changes_nothing` | 通過 |
| FR-18 履歴の全件置き換え | `history_store::history_store_replace_all` | 通過 |
| FR-11 履歴の読み込みと索引構築 | `history_store::history_store_append_and_load_round_trip` | 通過 |
| FR-22 20 件書き出して取り込む | `store::holiday_io_export_then_import` | 通過 |
| FR-22 解釈できない行の行番号を返す | `store::holiday_io_reports_malformed_lines` | 通過 |
| FR-22 0 件では書き出さない | `store::holiday_io_export_refuses_when_empty` | 通過 |
| FR-22 名称なしの行も受ける / 注記と空行を飛ばす | `store::holiday_io_accepts_date_without_name`, `store::holiday_io_skips_comments_and_blank_lines` | 通過 |
| FR-23 指定した記録のみ削除 | `history_store::history_store_remove_only_specified_entries` | 通過 |
| FR-23 削除で再び登録候補になる | `history_store::history_store_removed_date_becomes_candidate_again` | 通過 |
| FR-23 存在しないキーは何も壊さない | `history_store::history_store_remove_absent_key_changes_nothing` | 通過 |
| FR-17 日時・操作・対象・結果を記録 | `logger::logger_writes_required_fields` | 通過 |
| FR-17 20 件がそれぞれ記録される | `logger::logger_writes_one_line_per_record` | 通過 |
| FR-17 重大度の表示 | `logger::logger_levels_are_labelled` | 通過 |
| NFR-05 トークンは閉包経由のみで使える | `secret::secret_token_is_usable_only_through_closure`, `secret::secret_use_token_returns_none_when_unset` | 通過 |
| NFR-05 破棄するとトークンが使えない | `secret::secret_clear_removes_token` | 通過 |
| NFR-05 設定ファイルにトークンの場所がない | `store::document_settings_round_trip_without_secrets` | 通過 |
| NFR-07 保持しているトークンがログに出ない | `logger::logger_redacts_held_token` | 通過 |
| NFR-07 形から判断してトークンを伏せる | `logger::logger_redacts_unknown_token_patterns` | 通過 |
| NFR-07 表示に値を含めない | `secret::secret_debug_output_hides_values`, `secret::secret_proxy_credentials_debug_hides_password` | 通過 |
| NFR-07 伏せ字の前後を保つ / 秘密がなければ変えない | `logger::logger_redact_keeps_surrounding_text`, `logger::logger_redact_without_secret_is_unchanged` | 通過 |
| NFR-09 保存先がユーザープロファイル配下 | `paths::paths_from_environment_is_under_appdata` | 通過 |
| NFR-10 上限で世代が切り替わり 5 世代を保持 | `logger::logger_rotates_at_limit_and_keeps_generations` | 通過 |
| NFR-10 ローテーション後も書き続けられる | `logger::logger_continues_writing_after_rotation` | 通過 |
| NFR-11 1 つのフォルダ配下にまとまる | `paths::paths_all_files_live_under_one_root`, `store::document_all_files_under_one_folder` | 通過 |
| NFR-17 プロキシ資格情報も同じ経路で扱う | `secret::secret_proxy_credentials_usable_through_closure`, `secret::secret_redaction_targets_include_token_and_password` | 通過 |
| EC-04 同一日付は最初の 1 件のみ | `store::holiday_io_duplicate_dates_keep_first`, `store::holiday_io_duplicate_with_existing_is_reported` | 通過 |
| EC-07 初回起動で 0 件を返す | `store::document_first_run_returns_empty`, `atomic::schema_load_missing_file_is_none`, `history_store::history_store_load_missing_file_is_empty` | 通過 |
| EC-08 追記は 1 件ごとに確定する | `history_store::history_store_append_is_durable_per_entry`, `history_store::history_store_file_is_line_per_entry_and_readable` | 通過 |
| EC-08 中断の印で前回の中断を検知 | `store::in_progress_mark_is_set_and_cleared`, `store::in_progress_mark_detects_interruption`, `store::in_progress_clear_without_mark_succeeds` | 通過 |
| EC-27 削除は原子的置換で行い一時ファイルを残さない | `history_store::history_store_remove_leaves_no_temp_file`, `atomic::atomic_write_leaves_no_temp_file` | 通過 |
| EC-28 新しい版は読み込まず版を示して失敗 | `atomic::schema_rejects_newer_version` | 通過 |
| EC-28 現行版は読み込める / 形式不正は既存を変えない | `atomic::schema_accepts_current_version`, `atomic::schema_rejects_malformed_file` | 通過 |
| 設計書 5.3 置き換え先があっても上書きできる | `atomic::atomic_write_replaces_existing_file`, `atomic::atomic_write_creates_missing_parent_directory` | 通過 |
| 設計書 5.4 版付き文書の往復 | `atomic::schema_save_and_load_round_trip` | 通過 |
| 履歴の 1 行破損で全件を失わない | `history_store::history_store_skips_malformed_line_and_reports_it`, `history_store::history_store_ignores_blank_lines` | 通過 |
| EC-28 設定フォルダの複製先を決められる | `paths::paths_backup_root_is_sibling_of_root`, `paths::paths_ensure_created_makes_root_and_logs` | 通過 |

## ステップ

- [x] I1: infra クレートの骨格と依存追加 検証: `cargo build` が成功
- [x] I2: 保存先の決定 (ユーザープロファイル配下) [対応: NFR-09、NFR-11] 検証: `cargo test paths`
- [x] I3: 原子的置換とスキーマ版の判定 [対応: 設計書 5.3、5.4、EC-28] 検証: `cargo test atomic`, `cargo test schema`
- [x] I4: 設定・プロジェクト・定義・休日の読み書き [対応: FR-02、FR-03、FR-13、FR-18] 検証: `cargo test document`
- [x] I5: 履歴の追記と削除時の原子的置換 [対応: ADR-04、FR-11、FR-23、EC-27] 検証: `cargo test history_store`
- [x] I6: 一括登録の実行中を示す印 [対応: EC-08] 検証: `cargo test in_progress`
- [x] I7: 休日カレンダーの取り込みと書き出し [対応: FR-22、EC-04] 検証: `cargo test holiday_io`
- [x] I8: 定義と履歴のエクスポート・インポート [対応: FR-18] 検証: `cargo test transfer`
- [x] I9: Logger (ローテーションと秘密の除去) [対応: FR-17、NFR-07、NFR-10] 検証: `cargo test logger`
- [x] I10: SecretHolder [対応: NFR-05] 検証: `cargo test secret`
- [x] I11: スイート全体の実行と突合 検証: `cargo test` 全件成功

## 実行記録 2

- I1-I3: `cargo test -p gitlabregister-infra` → 失敗。バイト文字列リテラルに非 ASCII が混入 (ツールの JSON エスケープが実文字へ展開された)。通常の文字列に直して解消
- I1-I3: `cargo test -p gitlabregister-infra` → 13 passed; 0 failed (paths、atomic)
  - `fs::rename` が Windows でも置き換え先を上書きすることを `atomic_write_replaces_existing_file` で実測確認した (推測で済ませなかった)
- I5-I6、I9-I10: `cargo test -p gitlabregister-infra` → 42 passed; 0 failed (history_store、secret、logger)
- I4、I6-I8: `cargo test` → 145 passed; 0 failed (store を追加)
  - 内訳: コア層 78 + 性能 4 + インフラ層 63
- 完了検証: `cargo build` の警告 0 件。`dbg!`/`todo!`/`FIXME` の残置 0 件
- 完了検証: `secret.rs` にファイル書き込みの呼び出しが 1 件もないことを grep で確認 (NFR-05 の構造的保証)
- 完了検証: 対応表 2 の全 44 行が通ったテストを持つ

## 逸脱記録 2

### D-5: 休日の取り込み形式を定めた [分類: 実装の自由範囲]

FR-22 は取り込みと書き出しを求めるが形式を定めず、設計書 版 0.2 も定めていない。
1 行 1 件の `YYYY-MM-DD,名称` とした。先頭が `#` の行は注記、空行は読み飛ばす。
行単位にしたのは FR-22 の受け入れ基準が「解釈できなかった行の行番号を一覧表示する」を
求めており、行と記録が 1 対 1 でないと行番号を示せないためである。

### D-6: AppSettings と Project をインフラ層に置いた [分類: 実装の自由範囲]

設計書 5.1 はこの 2 つをエンティティとして挙げているが、計算に使う型ではなく
読み書きの対象でしかないため、コア層ではなくインフラ層に置いた。
コア層を「副作用のない計算」だけに保つという ADR-02 の方針に沿う。

### D-7: 履歴の 1 行破損で全件を失わせない [分類: 実装の自由範囲]

要件書は履歴ファイルの行破損を扱っていない。FR-18 で人がテキストエディタで編集できる
形式にしたため、手編集による破損が起こりうる。解釈できない行は読み飛ばして行番号を返し、
他の履歴は保つことにした。全件を失うより被害が小さい。

### D-8: SecretHolder の破棄は最善努力である [分類: 実装の自由範囲 / 報告対象]

`Drop` で保持している文字列を 0 で埋めるが、次の 2 点は保証できない。

- 値がこの保持器へ渡る前に作られた複製 (画面からの受け渡しの途中)
- OS がメモリをページファイルへ書き出した場合

FR-01 の受け入れ基準「端末上のいずれのファイルにも PAT が含まれない」は、
アプリがファイルへ書かないことで満たしている。本型はそれをさらに狭めるための措置である。
この限界はコードのコメントにも残した。


---

# 実装計画 3: GitLabRegister GitLab 通信層

## 今回の範囲

設計書 ADR-08 の GitLabClient。応答を 8 分類へ正規化し、再試行・待機・タイムアウト・プロキシの規則を 1 箇所に集める。

実インスタンスがないため、次の 2 件は暫定実装にして確認待ちとする (設計書 第 10 節 R-3、R-5)。

- イテレーション設定のエラー分類が 8 分類に落ちるか (R-3)
- ライセンスティアを接続時に判定できるか (R-5)

通信を伴わない部分 (分類、再試行、待機、要求の組み立て) は、応答を模した差し替え可能な
通信口を使って局所試験で検証する。試験サーバーを立てず、ネットワークも使わない。

## 追加する依存 (実装の自由範囲として決定、報告で明示)

| 依存 | 用途 | 選定理由 |
|---|---|---|
| reqwest 0.13 (既定機能 + json + query) | HTTP 通信 | 既定の rustls が OS の証明書ストア (native platform verifier) を使うため、社内の証明機関を OS に導入している環境で NFR-06 を満たす。`system-proxy` も既定で有効で、Windows のシステムプロキシ設定に従うため NFR-17 の既定動作を満たす |
| tokio | 非同期実行と再試行の待機 | Tauri が同じ実行時を使う。NFR-02 の同時実行数 2 を素直に書ける |

**重要**: 既定の rustls が束ねた CA (webpki-roots) を使う構成にすると、
社内の証明機関で署名された GitLab-EE へ接続できず NFR-06 を満たせない。
reqwest 0.13 の既定は OS の検証器を使うため、TLS の機能指定を変更してはならない。

## ステップ

- [x] G1: 依存追加と gitlab モジュールの骨格 検証: `cargo build` が成功
- [x] G2: 応答の 8 分類への正規化 [対応: 設計書 6.2、EC-11〜EC-14、EC-16〜EC-18、EC-29、EC-33、EC-34] 検証: `cargo test gitlab::error`
- [x] G3: 差し替え可能な通信口と reqwest 実装 [対応: ADR-08、NFR-06、NFR-17] 検証: `cargo test gitlab::transport`
- [x] G4: 再試行と待機 [対応: EC-16、EC-17、EC-18] 検証: `cargo test gitlab::retry`
- [x] G5: 接続確認・バージョン判定・ティア判定 [対応: FR-01、NFR-13、EC-15、EC-35] 検証: `cargo test gitlab::connect`
- [x] G6: Issue の作成 [対応: FR-06、FR-09、FR-12、EC-32、EC-33] 検証: `cargo test gitlab::issue`
- [x] G7: イテレーション設定 [対応: ADR-05、FR-06] 検証: `cargo test gitlab::iteration`
- [x] G8: プロジェクト検索と候補取得 [対応: FR-02、FR-16、EC-29] 検証: `cargo test gitlab::project`, `cargo test gitlab::candidate`
- [x] G9: スイート全体の実行と突合 検証: `cargo test` 全件成功

## 受け入れ基準 → テスト対応表 3

| 基準 (要件 ID) | テスト | 状態 |
| --- | --- | --- |
| 設計書 6.2 状態コードが 8 分類へ正規化される | `gitlab::error::error_classify_status_covers_all_kinds` | 通過 |
| 設計書 6.2 既知でない 4xx は入力エラー | `gitlab::error::error_classify_unknown_4xx_is_validation` | 通過 |
| 設計書 6.2 再試行する分類は 2 つに限る | `gitlab::error::error_only_transient_and_rate_limited_are_retryable` | 通過 |
| 設計書 8.1 全体を止める分類は 2 つに限る | `gitlab::error::error_only_unreachable_and_authentication_abort_batch` | 通過 |
| NFR-16 すべての分類に次に取れる操作がある | `gitlab::error::error_every_kind_has_next_action` | 通過 |
| NFR-07 応答のトークンが失敗の表示に残らない | `gitlab::error::error_message_redacts_token_in_response`, `gitlab::client::error_message_from_response_is_redacted` | 通過 |
| EC-33 項目ごとの検証エラーから項目名と理由を取り出す | `gitlab::error::error_extract_message_from_field_errors` | 通過 |
| 応答本文の各形から理由を取り出す | `gitlab::error::error_extract_message_from_plain_message`, `..._from_error_key`, `..._from_message_array`, `..._from_non_json`, `..._truncates_long_body` | 通過 |
| NFR-06 HTTPS でなければ通信口を作らない | `gitlab::transport::transport_rejects_non_https_url` | 通過 |
| NFR-06 証明書検証を無効化する経路がない | `cargo build` 後の grep で呼び出し 0 件 (実行記録) | 通過 |
| NFR-17 プロキシの 3 つの指定で通信口を作れる | `gitlab::transport::transport_builds_for_each_proxy_choice` | 通過 |
| NFR-17 プロキシの指定が不正なら失敗する | `gitlab::transport::transport_rejects_invalid_proxy` | 通過 |
| NFR-05 通信口はトークンを保持せず送るたびに取り出す | `gitlab::transport::transport_pulls_token_from_source_each_time` | 通過 |
| NFR-07 通信口の表示にトークンを含めない | `gitlab::transport::transport_debug_output_hides_token_source` | 通過 |
| EC-16 1 件あたり 30 秒で打ち切る | `gitlab::transport::transport_timeout_is_30_seconds` | 通過 |
| EC-16 応答が返らない場合は到達不能、再試行しない | `gitlab::client::retry_timeout_is_unreachable_and_not_retried` | 通過 |
| EC-17 一時エラーは 2 回まで再試行する | `gitlab::client::retry_transient_up_to_two_times` | 通過 |
| EC-17 再試行で成功したらその結果を返す | `gitlab::client::retry_transient_then_success` | 通過 |
| EC-18 待機時間の指示に従い 1 回再試行する | `gitlab::client::retry_rate_limited_once_with_retry_after` | 通過 |
| EC-18 要求上限が続けば 1 回で打ち切る | `gitlab::client::retry_rate_limited_gives_up_after_one_attempt` | 通過 |
| EC-18 待機時間の指示を秒数として取り出す | `gitlab::error::error_classify_rate_limited_keeps_retry_after` | 通過 |
| 設計書 6.2 既定の再試行規則が設計と一致する | `gitlab::client::retry_default_policy_matches_design` | 通過 |
| EC-34 プロキシ認証の失敗は再試行しない | `gitlab::client::retry_proxy_authentication_is_not_retried` | 通過 |
| EC-12 認可エラーは再試行しない | `gitlab::client::retry_authorization_is_not_retried` | 通過 |
| EC-11 認証の失敗は再試行しない | `gitlab::client::connect_authentication_failure_is_not_retried` | 通過 |
| FR-01 接続に成功すると利用者名が返る | `gitlab::client::connect_returns_username_and_version` | 通過 |
| NFR-13 バージョン下限の境界 (19.2 を満たす / 19.1 は満たさない) | `gitlab::client::connect_version_boundary` | 通過 |
| EC-35 下限未満のバージョンを判別できる | `gitlab::client::connect_detects_unsupported_version` | 通過 |
| EC-35 バージョンを解釈できない場合は失敗する | `gitlab::client::connect_rejects_unparsable_version` | 通過 |
| EC-15 ティアを判定できない場合は Unknown で EE 固有項目を許す | `gitlab::client::connect_tier_unknown_when_not_permitted` | 通過 |
| EC-15 Free と判定できた場合は EE 固有項目を許さない | `gitlab::client::connect_tier_free_disallows_ee_fields` | 通過 |
| FR-06 指定した項目が要求に載る / イテレーションは載せない | `gitlab::client::issue_create_sends_all_fields` | 通過 |
| FR-06 未設定の項目は要求に載せない | `gitlab::client::issue_create_omits_unset_fields` | 通過 |
| EC-33 マイルストーン不一致は入力エラー | `gitlab::client::issue_create_milestone_not_found_is_validation` | 通過 |
| EC-13 担当者を割り当てられない場合は入力エラー | `gitlab::client::issue_create_assignee_error_is_validation` | 通過 |
| EC-02 タイトルが長すぎる理由が利用者に伝わる | `gitlab::client::issue_create_title_too_long_message` | 通過 |
| ADR-05 作成とは別の操作でイテレーションを設定する | `gitlab::client::iteration_set_sends_mutation` | 通過 |
| ADR-05 状態コード 200 でも本文のエラーを拾う | `gitlab::client::iteration_set_mutation_errors_become_validation` | 通過 |
| ADR-05 最上位のエラーを分類へ落とす | `gitlab::client::iteration_set_top_level_errors_are_classified` | 通過 |
| FR-02 プロジェクトを取得できる | `gitlab::client::project_fetch_returns_path_with_namespace` | 通過 |
| EC-29 アーカイブ / 削除は対象不在として扱う | `gitlab::client::project_archived_is_not_found`, `gitlab::client::project_deleted_is_not_found` | 通過 |
| FR-16 種別ごとに候補の名前を取り出す | `gitlab::client::candidate_fetch_maps_name_per_kind` | 通過 |
| FR-16 件数を指定して取得する / 0 件でも失敗しない | `gitlab::client::candidate_fetch_requests_large_page`, `gitlab::client::candidate_fetch_empty_list` | 通過 |

## 実行記録 3

- G1: `cargo build` → 成功 (reqwest 0.13.5、tokio 1.53)
- G2-G3: `cargo test -p gitlabregister-infra` → 失敗。`RequestBuilder::query` が見つからない (reqwest 0.13 では `query` 機能が既定で無効)。機能を有効にして解消
- G2-G3: `cargo test` → 失敗。`ReqwestTransport` に `Debug` がなく `expect_err` が使えない。`Debug` を実装して解消
- G2-G3: `cargo test -p gitlabregister-infra` → 85 passed; 0 failed (error、transport)
- G4-G8: `cargo test -p gitlabregister-infra` → 115 passed; 0 failed (client を追加)
- 完了検証中に欠陥を検出: `ReqwestTransport` が `HttpTransport` を実装しておらず、**実通信の経路が繋がっていなかった**。差し替え実装でしか動かない状態だった
  - トークンの供給を `TokenSource` トレイトで表し、`RwLock<SecretHolder>` に実装。`Arc<RwLock<SecretHolder>>` をそのまま `Arc<dyn TokenSource>` として渡せる
  - `ReqwestTransport` は値を保持せず、送るたびに供給元から取り出す (NFR-05)
  - `TransportError::MissingToken` を追加し、認証エラーへ分類して再試行しないようにした
- 最終: `cargo test` → 199 passed; 0 failed (コア層 78 + 性能 4 + インフラ層 117)
- 最終: `cargo build` の警告 0 件。`dbg!`/`todo!`/`FIXME` の残置 0 件
- 最終: 証明書検証を無効化する呼び出し (`danger_accept_invalid_certs` 等) が 0 件であることを grep で確認 (NFR-06)

## 逸脱記録 3

### D-9: ライセンスティアを判定できない場合の扱い [分類: 設計への影響あり / 要承認]

ティアを直接示す情報は管理者権限がないと取得できない。権限のない利用者では判定できない。
EC-15 は「定義の編集画面で EE 固有項目を入力できない状態にする」と定めており、
事前にティアが分かることを前提にしている。

判定できない場合を `Tier::Unknown` として表し、要件書 第 3 節の前提 (Premium 以上) に従って
EE 固有項目を使える扱いにした。登録が失敗した場合に EC-15 の扱いへ落とす。

これは設計書 第 10 節 R-5 が予見していた分岐である。実インスタンスで
ティアを判定できるか確認し、できない場合はこの方針を設計書へ反映する必要がある。

### D-10: GraphQL の応答から分類を文面で推定する [分類: 実装の自由範囲 / 確認待ち]

GraphQL は状態コード 200 でも本文にエラーを入れる。設計書 6.2 の 8 分類へ落とすため、
最上位エラーの文面から分類を推定する実装にした (unauthorized → 認証、
permission → 認可、not found → 対象不在、その他 → 入力エラー)。

文面に依存するため、実インスタンスの応答で推定が合うかは未確認である
(設計書 第 10 節 R-3)。推定を外しても再試行しない分類に落ちるため、
誤って無限に再試行することはない。

### D-11: 再試行の待機時間を差し替え可能にした [分類: 実装の自由範囲]

設計書 6.2 は待機を 2 秒・4 秒・60 秒と定める。既定値はその通りにしたうえで、
試験では待機を 0 にできるようにした。待機を実時間で待つと試験が 66 秒かかり、
規則の検証に見合わない。

### D-12: トークンの写しは送信時に 1 つ作られる [分類: 実装の自由範囲 / 報告対象]

HTTP の見出しに値を載せるため、送信のたびにトークンの写しが 1 つ作られる。
これは避けられない。写しは要求を送り終えた時点で捨てられ、ファイルにもログにも渡らない。
通信口は値を保持せず、表示にも含めない (D-8 の限界と同じ性質)。


---

# 実装計画 4: GitLabRegister サービス層 (前半)

## 今回の範囲

設計書 4.2 のサービス層のうち、判断を持つ 3 つに限る。

- ConnectionService (FR-01、EC-11、EC-15、EC-35、NFR-17)
- ExpansionService (FR-07、FR-11、EC-05、EC-07、EC-08、EC-23、EC-32)
- RegistrationService (FR-06、FR-09、FR-10、FR-12、FR-20、EC-08、EC-10、EC-15、NFR-02)

ScheduleService・CandidateService・MasterDataService はコア層とインフラ層の組み合わせが
中心で判断が薄いため、コマンド境界と併せて次の計画で扱う。

設計書 4.1 はサービス層・コマンド境界・UI 層をアプリ本体として 1 つの実装単位に置くと
定めているため、新しいクレート `gitlabregister-app` に置く。UI は後からこのクレートに加える。

## ステップ

- [x] A1: app クレートの骨格 検証: `cargo build` が成功
- [x] A2: ConnectionService [対応: FR-01、EC-11、EC-35、NFR-17] 検証: `cargo test connection`
- [x] A3: ExpansionService [対応: FR-07、FR-11、EC-05、EC-07、EC-23、EC-32] 検証: `cargo test expansion`
- [x] A4: EC-08 の中断検知をプレビューの警告に出す [対応: EC-08] 検証: `cargo test expansion_warns_interrupted`
- [x] A5: RegistrationService の逐次登録と履歴追記 [対応: FR-09、FR-10、EC-08] 検証: `cargo test registration`
- [x] A6: イテレーションの 2 段目と部分成功 [対応: FR-06、ADR-05] 検証: `cargo test registration_iteration`
- [x] A7: 中止と全体停止の分類 [対応: FR-09、EC-10、EC-11] 検証: `cargo test registration_abort`
- [x] A8: 失敗分の再実行 [対応: FR-20] 検証: `cargo test registration_retry`
- [x] A9: 単発登録 [対応: FR-12] 検証: `cargo test registration_single`
- [x] A10: EC-15 の EE 固有項目の除外 [対応: EC-15] 検証: `cargo test registration_tier`
- [x] A11: 同時実行数 2 [対応: NFR-02] 検証: `cargo test registration_concurrency`
- [x] A12: スイート全体 検証: `cargo test` 全件成功

## 受け入れ基準 → テスト対応表 4

| 基準 (要件 ID) | テスト | 状態 |
| --- | --- | --- |
| FR-01 接続に成功すると利用者名とバージョンが状態に入る | `connection::connection_connect_sets_connected_state` | 通過 |
| FR-01 接続先 URL が未入力なら通信口を作らない | `connection::connection_prepare_transport_requires_url` | 通過 |
| FR-01 / NFR-05 トークンは SecretHolder へ渡り、サービスは保持しない | `connection::connection_prepare_transport_stores_token_in_holder_only` | 通過 |
| NFR-06 HTTPS でない接続先を拒否する | `connection::connection_prepare_transport_rejects_non_https` | 通過 |
| NFR-17 プロキシの資格情報も SecretHolder へ渡る | `connection::connection_prepare_transport_stores_proxy_credentials` | 通過 |
| NFR-17 3 つの指定が対応する選択へ変換される | `connection::connection_proxy_mode_maps_to_choice` | 通過 |
| NFR-17 手動指定でホストが欠けていれば OS 構成に従う | `connection::connection_manual_proxy_without_host_falls_back_to_system` | 通過 |
| EC-11 認証の失敗で未接続へ戻り秘密情報を破棄する | `connection::connection_authentication_failure_clears_secrets`, `connection::connection_mark_authentication_lost_clears_secrets` | 通過 |
| EC-15 ティア不明では注意を出すが EE 固有項目を許す | `connection::connection_unknown_tier_warns_but_allows_ee_fields` | 通過 |
| EC-15 Free では EE 固有項目を許さない | `connection::connection_free_tier_disallows_ee_fields` | 通過 |
| EC-19 到達不能では登録できないが利用者名を残す | `connection::connection_mark_unreachable_keeps_username` | 通過 |
| EC-35 下限未満のバージョンでは登録できない | `connection::connection_unsupported_version_blocks_registration` | 通過 |
| 未接続時に可能な操作の範囲 | `connection::connection_disconnected_capabilities` | 通過 |
| FR-07 期間を指定すると定義が展開される | `expansion::expansion_expands_enabled_schedules` | 通過 |
| FR-07 期間の逆転を拒否する | `expansion::expansion_rejects_reversed_range` | 通過 |
| FR-11 履歴にある発生日を除外し除外一覧に載せる | `expansion::expansion_excludes_registered_dates` | 通過 |
| FR-09 登録済みを索引へ加えると次の展開で除外される | `expansion::expansion_record_registered_excludes_next_time` | 通過 |
| FR-14 無効化した定義は展開されない | `expansion::expansion_all_disabled_reports_no_enabled_schedules` | 通過 |
| FR-23 履歴削除後に索引を捨てると再び候補になる | `expansion::expansion_invalidate_history_reflects_deletion` | 通過 |
| EC-05 上限超過を警告し一括登録を実行できない | `expansion::expansion_warns_too_many_candidates` | 通過 |
| EC-07 定義 0 件でも展開は成功し 0 件と分かる | `expansion::expansion_first_run_has_no_schedules` | 通過 |
| EC-08 中断の印から重複確認を促す警告を出す | `expansion::expansion_warns_interrupted_registration` | 通過 |
| EC-23 休日未登録の年を警告する | `expansion::expansion_warns_year_without_holidays` | 通過 |
| EC-24 プリセットは渡された基準日から求める | `expansion::expansion_resolve_preset_uses_given_today` | 通過 |
| EC-32 候補にないラベルを警告し新規作成される旨を伝える | `expansion::expansion_warns_unknown_labels` | 通過 |
| EC-32 候補が揃っていれば警告を出さない | `expansion::expansion_no_label_warning_when_all_known` | 通過 |
| EC-32 突合できなかったことを区別して伝える | `expansion::expansion_reports_labels_not_verified` | 通過 |
| EC-32 ラベル未指定では突合の警告を出さない | `expansion::expansion_no_label_warning_without_labels` | 通過 |
| 履歴の破損行を警告し展開は続ける | `expansion::expansion_warns_malformed_history_lines` | 通過 |
| FR-09 選択された件数だけ登録し履歴に追記する | `registration::registration_registers_all_items` | 通過 |
| FR-09 1 件の処理完了ごとに進捗を通知する | `registration::registration_reports_progress_per_item` | 通過 |
| FR-09 中止すると未処理分は登録せず登録済みは残る | `registration::registration_cancel_stops_remaining` | 通過 |
| FR-10 件ごとの成否と失敗理由が返る | `registration::registration_reports_per_item_outcome` | 通過 |
| 設計書 8.1 入力エラーでは全体を止めない | `registration::registration_validation_error_does_not_abort` | 通過 |
| EC-10 通信切断で処理を止め件数が分かる | `registration::registration_abort_on_unreachable` | 通過 |
| EC-11 認証失効で全体を止める | `registration::registration_abort_on_authentication_lost` | 通過 |
| FR-06 / ADR-05 作成後に別操作でイテレーションを設定する | `registration::registration_iteration_set_after_create` | 通過 |
| FR-06 イテレーション設定のみ失敗は成功扱いで併記する | `registration::registration_iteration_failure_keeps_success` | 通過 |
| ADR-05 イテレーション未指定なら 1 通信で済ませる | `registration::registration_without_iteration_uses_single_request` | 通過 |
| EC-15 ティアが許さない EE 固有項目を外し項目名を返す | `registration::registration_tier_drops_ee_fields` | 通過 |
| FR-20 失敗した行のみを再実行し成功済みは処理しない | `registration::registration_retry_failed_only` | 通過 |
| FR-20 失敗 0 件では再実行を実行できない | `registration::registration_retry_not_available_without_failures` | 通過 |
| FR-12 単発登録は履歴に記録しない | `registration::registration_single_does_not_record_history` | 通過 |
| FR-12 単発登録の失敗理由を返す | `registration::registration_single_returns_error` | 通過 |
| EC-08 開始で印を残し終了時に消す | `registration::registration_clears_in_progress_mark_on_finish` | 通過 |
| EC-08 全体停止で終わっても印を消す | `registration::registration_clears_mark_even_when_aborted` | 通過 |
| NFR-02 既定の同時実行数が 2 で設計書 8.4 と一致する | `registration::registration_default_concurrency_is_two` | 通過 |
| NFR-02 2 件が同じ組で進み次の組は止まる | `registration::registration_concurrency_processes_two_at_a_time` | 通過 |
| NFR-02 奇数件でもすべて処理される | `registration::registration_concurrency_handles_odd_counts` | 通過 |
| NFR-16 失敗の表示に使う識別情報が揃う | `registration::registration_item_describe_includes_name_date_project` | 通過 |
| 0 件の一括登録は何もせず返る | `registration::registration_empty_batch` | 通過 |

## 実行記録 4

- A1: `crates/app/src/lib.rs` の作成漏れで `cargo` がマニフェストを読めず失敗。lib.rs を作って解消
- A1: `FakeTransport` が `#[cfg(test)]` でクレート外から使えなかった。インフラ層の `testing` モジュール (機能フラグ `testing`) へ移し、app の開発依存で有効化
- A2: `cargo test -p gitlabregister-app` → 14 passed; 0 failed (ConnectionService)
- A3-A4: `cargo test -p gitlabregister-app` → 30 passed; 0 failed (ExpansionService)
- A5-A11: `cargo test -p gitlabregister-app` → 失敗。`GitLabClient` に通信口を参照する経路がなかった。`transport()` を追加して解消
- A5-A11: `cargo test -p gitlabregister-app` → 52 passed; 0 failed (RegistrationService)
- A12: `cargo test` → 251 passed; 0 failed (コア層 78 + 性能 4 + インフラ層 117 + アプリ層 52)
- A12: `cargo build` の警告 0 件。`dbg!`/`todo!`/`FIXME` の残置 0 件
- A12: サービス層にトークンを保持するフィールドがないことを grep で確認 (NFR-05)

## 逸脱記録 4

### D-13: 同時実行は「組」単位で進める [分類: 実装の自由範囲]

設計書 8.4 は同時実行数 2 と定めるが、進め方は定めていない。
外部の依存を増やさないため、2 件ずつの組に分けて合流させる形にした。
組の境目で待ち合わせが入るため、理論上の最良値よりわずかに遅い。
全件が 2 通信の最悪条件 (1 通信 1 秒) で 50 組 × 2 秒 = 100 秒であり、
NFR-02 の 180 秒には収まる。

この形の副作用として、全体停止の判定は「同じ組の 2 件目は処理される」ことになる。
EC-10 は「切断時点で処理を止める」と定めるが、同時に進んでいる 1 件を
途中で捨てる規定はないため、組の完了を待って止める扱いにした。
試験 `registration_concurrency_processes_two_at_a_time` でこの振る舞いを固定している。

### D-14: 中断の印は中止・全体停止でも消す [分類: 設計への影響あり / 要承認]

EC-08 の印は「前回の一括登録が中断されたこと」を次回起動時に知らせるためのものである。
利用者が意図して中止した場合と、全体停止で終わった場合も、処理は最後まで終わっている
(結果一覧が出ている) ため、印を残すと次回に不要な警告が出る。
このため正常終了・中止・全体停止のいずれでも印を消すことにした。

印が残るのは「プロセスが落ちた」場合だけになる。これは EC-08 が想定する状況
(強制終了・電源断) と一致するが、設計書 7.2 は「正常終了時に消す」としか書いていない。
承認されれば設計書 7.2 に追記する。

### D-15: ラベルの突合は候補を取得済みのプロジェクトに限る [分類: 実装の自由範囲]

EC-32 は未登録のラベル名を警告することを求めるが、候補を取得していない場合の扱いを
定めていない。突合できなかったことを別の警告 (`LabelsNotVerified`) として区別し、
「ラベルは正しい」と誤解させないようにした。

### D-16: 試験用の差し替え実装を機能フラグで公開した [分類: 実装の自由範囲]

サービス層からインフラ層の判断を確かめるため、`FakeTransport` をインフラ層の
`testing` モジュールへ移し、`testing` 機能を有効にしたときだけ公開するようにした。
本番の配布には含まれない。


---

# 実装計画 5: サービス層 (後半) とコマンド境界

## 範囲

- ScheduleService (FR-03、FR-14、FR-21、EC-01〜EC-03、EC-06、EC-26、EC-30、EC-36)
- CandidateService (FR-16、EC-19、ADR-07)
- MasterDataService (FR-02、FR-13、FR-18、FR-19、FR-22、FR-23、EC-04、EC-27)
- コマンド境界 Boundary (設計書 6.1、EC-09、NFR-16)

## ステップ

- [x] B1: 候補値の保持をインフラ層へ追加 [対応: FR-16、設計書 5.1] 検証: `cargo test document_candidates`
- [x] B2: ScheduleService [対応: FR-03、FR-14、FR-21、EC-26、EC-30] 検証: `cargo test schedule`
- [x] B3: CandidateService [対応: FR-16、EC-19] 検証: `cargo test candidate`
- [x] B4: MasterDataService [対応: FR-02、FR-13、FR-18、FR-19、FR-22、FR-23] 検証: `cargo test masterdata`
- [x] B5: コマンド境界 [対応: 設計書 6.1、EC-09、NFR-16] 検証: `cargo test boundary`
- [x] B6: 要件 ID のカバレッジ検査と穴埋め 検証: 全要件 ID の言及を grep で確認
- [x] B7: スイート全体 検証: `cargo test` 全件成功

## 実行記録 5

- B1: `cargo test -p gitlabregister-infra` → 118 passed; 0 failed (候補値の保持を追加)
- B2-B3: `cargo test -p gitlabregister-app` → 80 passed; 0 failed (ScheduleService、CandidateService)
- B4-B5: `cargo test -p gitlabregister-app` → 114 passed; 0 failed (MasterDataService、Boundary)
- B6: 要件 ID 77 件をコードに対して grep。8 件が未言及だった
  - **EC-14 (担当者のアカウントが無効・削除) は試験が存在しなかった**。EC-13 と同じ分類になることを
    確かめる試験を追加した
  - NFR-03、NFR-08、NFR-14 は実現しているが根拠の記載がなかったため、該当箇所に明記した
  - NFR-12、NFR-15、NFR-18 と EC-25 は未言及のままとした。前 3 件は UI 層・配布に属し未実装、
    EC-25 は要件書が「該当なし」と定めたものでコードを必要としない
- B7: `cargo test` → 315 passed; 0 failed
  - 内訳: コア層 78 + 性能 4 + インフラ層 119 + アプリ層 114
- B7: `cargo build` の警告 0 件。`dbg!`/`todo!`/`FIXME` の残置 0 件

## 逸脱記録 5

### D-17: 複製時の名称は「〜 のコピー」とした [分類: 実装の自由範囲]

FR-03 は「名称の末尾に重複しない文字列が付く」としか定めていない。
`〜 のコピー`、既にあれば `〜 のコピー 2` と採番する形にした。

### D-18: 履歴の一覧は登録日時の降順で返す [分類: 実装の自由範囲]

FR-19 は並び順を定めていない。新しいものから見たいのが自然なため降順にした。

### D-19: プロジェクトの参照数は識別子と表示名の両方で数える [分類: 実装の自由範囲 / 報告対象]

FR-02 は「参照している定義の件数」を示すことを求める。
定義が持つ登録先は文字列であり、利用者が識別子で書く場合と名前空間付きの経路で書く場合がある。
どちらでも数えられるように両方と照合した。

ただしこれは**定義とプロジェクトの結び付きが文字列である**ことに起因する緩さである。
UI 層で登録先を一覧から選ばせる形にすれば解消するため、UI の実装時に見直す。

### D-20: コマンド境界が失敗を 8 種類の分類へまとめた [分類: 実装の自由範囲]

設計書 6.1 は「UI 層はこの分類だけを見て表示を切り替え、分類の判定を自分で行わない」と定めるが、
分類の一覧は定めていない。入力・認証・権限・対象不在・到達不能・一時的・保存・実行中の 8 つにした。
GitLab 側の 8 分類 (設計書 6.2) とは対応するが同一ではない。
保存の失敗と実行中 (EC-09) は GitLab 側に対応がないため、境界で足している。


---

# 実装計画 6: UI 層 (Tauri の外殻と画面)

## 範囲

設計書 4.2 の UI 層とコマンド境界の接続。

- Tauri の外殻 (窓の起動、WebView2 の封じ込め)
- Tauri コマンドとして Boundary を公開する
- 画面: 接続、定義一覧・編集、プレビュー、結果一覧、履歴・休日・プロジェクト管理
- NFR-15 (キーボード完結)、NFR-18 (表示スケール 100〜200%)、FR-08 (500 行のインライン編集)

## 方針

フロントエンドはビルド工程を持たない素の HTML / CSS / JavaScript とする。
ADR-01 の欠点として挙げた「ツールチェーンが 3 つ」が 2 つ (Rust と WebView2) に減り、
npm が不要になる。500 行 × 10 列の表は仮想化なしで DOM に出して計測し、
足りなければ仮想化を足す (設計書 R-6)。

## ステップ

- [x] U1: desktop クレートと Tauri の設定、窓が開くこと 検証: `cargo build -p gitlabregister-desktop` が成功
- [x] U2: WebView2 のユーザーデータフォルダを設定フォルダ配下へ固定 [対応: NFR-05、NFR-09] 検証: 起動後にフォルダが設定フォルダ配下に作られる
- [x] U3: 自動入力の無効化 [対応: NFR-05] 検証: 起動して設定フォルダを PAT で全文検索し一致 0 件
- [x] U4: Tauri コマンドで Boundary を公開 [対応: 設計書 6.1] 検証: `cargo build` が成功
- [x] U5: 接続画面 [対応: FR-01、NFR-17] 検証: 画面から接続できる
- [x] U6: 定義一覧と編集画面 [対応: FR-03、FR-04、FR-05、FR-06、FR-13、FR-14、FR-21] 検証: 定義を作成・編集できる
- [x] U7: プレビュー画面と 500 行のインライン編集 [対応: FR-07、FR-08、R-6] 検証: 500 行で操作の応答を計測
- [x] U8: 一括登録の実行と結果一覧 [対応: FR-09、FR-10、FR-20] 検証: 進捗と結果が表示される
- [x] U9: 履歴・休日・プロジェクトの管理画面 [対応: FR-02、FR-13、FR-19、FR-22、FR-23] 検証: 各操作が画面から行える
- [x] U10: 単発登録 [対応: FR-12] 検証: 画面から 1 件登録できる
- [x] U11: 表示スケールとキーボード操作 [対応: NFR-15、NFR-18] 検証: 100〜200% で表示が欠けない
- [x] U12: 起動時間の計測 [対応: NFR-03] 検証: 起動から接続画面の操作まで 5 秒以内
- [x] U13: フォルダ展開での起動確認 [対応: NFR-12、R-1] 検証: 展開のみで起動する

## 実行記録 6

- U1: `cargo build` → 失敗。`icons/icon.ico` がないと Windows のリソース生成ができない。32x32 の ICO を生成して解消
- U1: `cargo build` → 失敗。`frontendDist` の相対基準を誤っていた (設定ファイルの位置が基準)。`ui` に直して解消
- U1: `cargo build` → 失敗。`chrono::Local` が使えない。`clock` 機能を切っているためで、これは ADR-06 の構造的保証そのもの。機能を有効にせず、外殻に `clock` モジュールを置いて日本標準時の固定オフセットで現在時刻を取る形にした (NFR-14)
- U1: `cargo build -p gitlabregister-desktop` → 成功、警告 0 件
- U2: 起動して `%APPDATA%\GitLabRegister\webview\EBWebView` が作られることを実機で確認。
  **WebView2 のユーザーデータフォルダを設定フォルダ配下へ固定できることが確かめられた** (NFR-05、NFR-09、設計書 8.3 の未検証だった前提)
- U4-U5: 起動して接続画面を撮影。画面が描画され、`load_settings` と `connection_state` の往復が成功していることを確認
  (未接続の状態説明が表示されている = JS → Tauri コマンド → コマンド境界 → サービス層が通しで動く)
- U7: **欠陥を検出**。展開前なのに「一括登録する」が押せる状態だった (FR-07 の「0 件では実行できない」に反する)。
  画面の表示時と起動時に状態を評価するよう修正。修正後は「選択 0 件 / 全 0 件」とともに無効化されることを撮影で確認
- U7: 定義 6 件 (毎日) を仕込み、今四半期 (92 日) を展開 → **552 行**を描画。撮影で次を確認
  - EC-05: 「登録候補が 552 件で上限 500 件を超えました」の警告が出て「一括登録する」が無効
  - EC-32: 「候補を取得していないため、ラベルの存在を確認できませんでした」の警告が出る
  - FR-15: タイトルが「2026年10月1日 の確認1」、説明が「発生日 2026-10-01 / 期限 …」と差し込み済み
  - FR-07: 今四半期のプリセットが 2026-10-01〜2026-12-31 を設定
  - FR-08: 各行にタイトル・説明・ラベル・マイルストーンの編集欄が並ぶ
  - アプリのログに `展開 | 2026-10-01 〜 2026-12-31 | 登録候補 552 件` が記録される (FR-17)
- U7: 今月 (31 日 x 6 = 186 行) に切り替えると EC-05 の警告が消えることを確認。
  未接続のため「一括登録する」は無効のまま (EC-19 の縮退が効いている)
- U11: 主要な操作にアクセスキーを付けた (Alt+C 接続 / Alt+E 展開 / Alt+R 一括登録 / Alt+X 中止 / Alt+N 新規作成)。
  焦点の位置によらず実行でき、キーボードだけで操作を完了できる (NFR-15)
- U12: **NFR-03 の実測**。release 版を単体で起動し、窓が出るまでを 3 回計測
  - 428 ms (初回) / 70 ms / 73 ms。要件の 5 秒以内に対して十分な余裕がある
- U13: **NFR-12 の実測**。release 版の実行ファイル 1 つ (14.4 MB) を空のフォルダへ複製して起動を確認。
  インストーラなし、管理者権限なし、WebView2 の明示的な導入なしで起動した (設計書 R-1 の確認)
- 片付け: 仕込んだ試験データ (`%APPDATA%\GitLabRegister`) を削除した
- `cargo test` → 316 passed; 0 failed。`cargo build` の警告 0 件

### 計測の道具で踏んだ問題 (成果物とは無関係)

- PowerShell 5.1 は BOM なし UTF-8 を ANSI として読むため、日本語のコメントを含むスクリプトが構文誤りになる。
  計測スクリプトは ASCII のみで書くこと
- `Process.MainWindowHandle` がアプリの 16x16 の補助窓を指すことがある。
  窓はタイトルと大きさで特定する必要がある。これを誤ったまま座標クリックを送ると、
  画面上の無関係な位置を押して窓の大きさが壊れる。**アプリの不具合ではない**

## 逸脱記録 6

### D-22: フロントエンドをビルド工程なしにした [分類: 実装の自由範囲]

素の HTML / CSS / JavaScript (ES モジュール) で書き、npm と束ね処理を使わない。
ADR-01 の欠点として挙げた「ツールチェーンが 3 つ」が 2 つ (Rust と WebView2) に減った。
画面の規模に対して枠組みを入れる利得がないためでもある。

### D-23: 主要な操作にアクセスキーを付けた [分類: 実装の自由範囲]

NFR-15 は「キーボードのみでも完了できる」ことを求める。
タブ順だけで到達できるが、長い表の中から操作へ戻るのに手数がかかる。
Alt+C (接続) / Alt+E (展開) / Alt+R (一括登録) / Alt+X (中止) / Alt+N (新規作成) を割り当て、
焦点の位置によらず実行できるようにした。表示にも割り当てを併記している。

### D-24: 現在時刻は外殻で固定オフセットから作る [分類: 実装の自由範囲]

`chrono` の `clock` 機能を有効にすると、機能の統合によってコア層でも
`chrono::Local` が使えるようになり、ADR-06 の「コア層は時計を読めない」という
構造的な保証が失われる。機能を有効にせず、外殻の `clock` モジュールで
`SystemTime` から日本標準時の固定オフセット (+9 時間) を足して現在時刻を作る。
日本は夏時間を持たないため固定で足りる。OS の時刻帯設定には従わない。
従うと、端末の設定を変えただけで発生日の算出結果が変わりうる (NFR-14)。

### D-21: Tauri の外殻を別クレートにした [分類: 設計への影響あり / 要承認]

設計書 4.1 はサービス層・コマンド境界・UI 層をアプリ本体として 1 つの実装単位に置くと定める。
サービス層とコマンド境界は `gitlabregister-app` に置いたが、Tauri の外殻は
`gitlabregister-desktop` として分けた。

理由は試験の速さである。同じクレートに入れると `cargo test` のたびに Tauri を
コンパイルすることになり、315 件の試験が GUI のツールチェーンに依存する。
サービス層を GUI なしで試験できる状態を保ちたい。

承認されれば設計書 4.1 の表を 2 行に分ける。
