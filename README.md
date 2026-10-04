# GitLabRegister

日次・週次・月次で繰り返し発生する作業を、GitLab-EE の Issue としてまとめて起票する Windows デスクトップアプリです。

繰り返しタスクを一度「定義」として登録しておけば、期間を指定するだけで該当する発生日の Issue を一括で登録できます。
登録前にプレビューで内容を確認・編集でき、登録済みの発生日は履歴から除外されるため、同じ期間を二度登録しても Issue は重複しません。

## 主な機能

- 7 種類の繰り返しパターン (毎日、毎営業日、N 日ごと、毎週・N 週ごと、毎月 日付指定、毎月 第 N 曜日指定、毎年)
- 休日カレンダーに基づく営業日調整 (前後の営業日へ移動、またはその回を飛ばす)
- 期間指定による展開とプレビュー、プレビュー上での個別編集
- 一括登録、失敗分の再実行、登録履歴による重複除外
- 単発 Issue の登録
- タイトル・説明への発生日の差し込み
- 担当者・ラベル・マイルストーン・エピック・イテレーションなどの候補値を GitLab-EE から取得して選択

## 動作環境

- Windows 11 (64 ビット版)
- Microsoft Edge WebView2 ランタイム (Windows 11 には標準で導入済み)
- 接続先の GitLab-EE と、`api` スコープを持つパーソナルアクセストークン (PAT)

管理者権限は不要です。

## インストール

1. [Releases](../../releases) から最新の `GitLabRegister-<バージョン>-windows-x64.zip` をダウンロードします。
2. 任意のフォルダに展開し、`GitLabRegister.exe` を実行します。

インストーラはありません。
レジストリへの書き込みも行いません。
アンインストールするには、展開したフォルダを削除します。

ダウンロードしたファイルが改ざんされていないことは、同じ場所にある `.sha256` ファイルの値と、次のコマンドの結果を比べて確認できます。

```powershell
Get-FileHash .\GitLabRegister-<バージョン>-windows-x64.zip -Algorithm SHA256
```

> [!NOTE]
> 実行ファイルにはコード署名をしていません。
> 初回起動時に Microsoft Defender SmartScreen の警告が出た場合は、「詳細情報」から「実行」を選んでください。

## データの保存場所

定義、休日カレンダー、登録履歴、ログは `%APPDATA%\GitLabRegister` に保存されます。
PAT は端末に保存せず、起動のたびに入力を求めます。

設定ごと完全に削除したい場合は、展開したフォルダに加えてこのフォルダも削除してください。

## 開発

### 必要なもの

- Rust 1.90 以上 (stable)
- Windows 11 と WebView2 ランタイム

### ビルドと実行

```powershell
# 開発用に起動する
cargo run -p gitlabregister-desktop

# テストを実行する
cargo test --workspace

# 配布用の実行ファイルを作る (画面を exe に埋め込む)
cargo build --release -p gitlabregister-desktop --features custom-protocol
```

配布用の実行ファイルは `target\release\gitlabregister-desktop.exe` に出力されます。

### 構成

| クレート | 役割 |
| --- | --- |
| `crates/core` | 繰り返し展開、営業日調整、テンプレート差し込みなどのドメインロジック |
| `crates/infra` | GitLab API クライアント、ファイル保存、ログ |
| `crates/app` | ユースケース (展開、登録、マスターデータ取得など) |
| `crates/desktop` | Tauri の外殻と画面 (`ui/`) |

要件と設計は [docs/](docs/) にあります。

## リリース手順

1. `Cargo.toml` の `[workspace.package]` の `version` と、`crates/desktop/tauri.conf.json` の `version` を新しいバージョンに更新します。
2. `CHANGELOG.md` の `[Unreleased]` の内容を `## [x.y.z] - YYYY-MM-DD` の節に移します。
3. 変更をコミットし、`main` に push します。
4. タグを作って push します。

   ```powershell
   git tag v0.1.0
   git push origin v0.1.0
   ```

GitHub Actions ([release.yml](.github/workflows/release.yml)) がテスト、ビルド、zip の作成を行い、`CHANGELOG.md` の該当節をリリースノートにして GitHub Release を公開します。
タグとファイル内のバージョンが一致しない場合、または `CHANGELOG.md` に該当節がない場合は失敗します。
`v1.0.0-rc.1` のようにハイフンを含むタグはプレリリースとして公開されます。

## ライセンス

[MIT License](LICENSE)
