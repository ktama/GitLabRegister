# 変更履歴

このファイルには利用者に見える変更を記録する。
書式は [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) に、バージョン番号は [Semantic Versioning](https://semver.org/lang/ja/) に従う。
リリース時は `## [x.y.z]` の節の本文が GitHub Release のリリースノートになる。

## [Unreleased]

## [0.1.0] - 2026-10-03

### 追加

- GitLab-EE への接続 (URL と PAT を起動のたびに入力し、PAT は端末に保存しない)
- 登録先プロジェクトの管理
- 繰り返しタスク定義の作成・編集・複製・削除、有効・無効の切り替え、有効期間の指定
- 7 種類の繰り返しパターン (毎日、毎営業日、N 日ごと、毎週・N 週ごと、毎月 日付指定、毎月 第 N 曜日指定、毎年)
- 休日カレンダーと営業日調整、休日カレンダーの取り込み・書き出し
- 期間指定による発生日の展開、プレビュー上での個別編集と選択解除
- 一括登録、登録結果の表示、失敗した候補の再実行
- 登録履歴による重複除外、履歴の閲覧・検索・個別削除
- 単発 Issue の登録
- タイトルと説明への発生日の差し込み
- GitLab-EE 上の候補値 (担当者、ラベル、マイルストーンなど) の取得と選択
- 定義と履歴のエクスポート・インポート、操作ログの出力

[Unreleased]: https://github.com/ktama/GitLabRegister/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ktama/GitLabRegister/releases/tag/v0.1.0
