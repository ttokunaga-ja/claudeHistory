# claudeHistory

同じPCでClaude Desktopのアカウントを切り替えた後、Codeのローカル会話履歴を新しいアカウントの一覧へ追加するRust製CLIです。非公式ツールで、Anthropicによる承認・保証はありません。

macOS（Apple Silicon）とWindows（x64）に対応しています。通常チャット、Cowork、クラウド会話の移行、自動ログアウト・ログインには対応しません。履歴はローカルで処理し、認証情報や会話を外部へ送信しません。復元した会話で後からメッセージを送信すると、その会話の文脈はログイン中のアカウントでClaudeに送信されます。自分が所有し、移行してよいアカウント間だけで使用してください。

## インストール

OSに合うコマンドを1回実行します。最新の正式リリースを自動ダウンロードし、SHA-256と実行ファイルの版を確認してからインストールします。実行するフォルダーを選ぶ必要はありません。Rust・Python・管理者権限は不要です。

### macOS（Apple Silicon / ターミナル）

```sh
curl -fsSL https://raw.githubusercontent.com/ttokunaga-ja/claudeHistory/main/install.sh | sh
```

`~/.local/bin/claudeHistory`へインストールし、使用しているシェルの設定ファイル（zshなら`~/.zshrc`）へPATHの設定を追加します。**新しいターミナルを開く**と`claudeHistory`を使えます。Intel Mac向けの配布はありません。

### Windows x64（PowerShell）

```powershell
irm https://raw.githubusercontent.com/ttokunaga-ja/claudeHistory/main/install.ps1 | iex
```

`%USERPROFILE%\.local\bin\claudeHistory.exe`へインストールし、ユーザーPATHと現在のPowerShellのPATHへ追加します。そのまま`claudeHistory`を使えます。

```sh
claudeHistory --version
claudeHistory --help
```

チェックサムや実行ファイルの検証に失敗した場合は、既存の実行ファイルを置き換えません。更新時も同じインストールコマンドを使います。配布パッケージを手動で取得する場合は[Releases](https://github.com/ttokunaga-ja/claudeHistory/releases/latest)からダウンロードし、展開したフォルダー内の`install.sh`または`install.ps1`を実行してください。展開コマンドはダウンロード先のフォルダーで実行する必要があります。

## 使い方

Desktopでアカウントを切り替えた後に実行します。以下は両OS共通です。

```sh
claudeHistory accounts
claudeHistory status
claudeHistory transfer --dry-run
claudeHistory
```

移行元が1つなら自動選択し、複数なら番号で選びます。移行先はDesktop設定の現在のアカウント候補です。組織が複数ある場合は必ず選択します。IDと件数を確認してください。CLIログインのアカウントをDesktopの移行先として推測しません。

引き継ぎは不足分だけを追加します。同じIDの既存履歴は上書きしません。同じIDで会話参照や作業フォルダーが異なる場合は中止します。既存登録の古い表示情報の同期は対象外です。元の登録とバックアップを保持し、新しい登録の実行許可は手動にします。MCP接続、旧アカウントの許可、remote control、認証、定期実行設定は引き継ぎません。

```sh
claudeHistory transfer --from ACCOUNT_UUID --from-org SOURCE_ORG_UUID --to-org TARGET_ORG_UUID
claudeHistory undo /absolute/path/to/backup
```

`undo`は指定の実行で追加した登録だけを取り消します。追加後に変更された登録があれば中止します。会話本文と元の履歴は削除しません。

## 作業中の扱い

| 検出結果 | 動作 |
| --- | --- |
| Desktop・CLIとも終了済み | そのまま引き継ぎ |
| Claude CLI、作業用子プロセス、観測中の履歴更新あり | 「作業中です。作業を終了してから進めてください。」と表示して中止 |
| 起動中のDesktopで、作業の兆候を観測しなかった | 終了のYes/Noを確認。Yesの場合だけ終了を要求し、終了完了後に引き継ぎ |
| TCP接続あり、観測失敗、未知の関連プロセス | 安全に判定できないことを表示して中止 |

通信内容は読まず、関連プロセスのTCP接続を確認します。待機接続と作業中の通信を完全には区別できないため、接続があるときは保守的に中止します。対話待ちのCLIや待機中のMCPプロセスも自動終了しません。アプリを手動終了してから再実行してください。No・Enter・入力終了では引き継ぎません。強制終了はしません。

短い観測では、ネットワーク待ち、権限回答待ち、観測直後に始まる作業などを完全に検出できません。Yes/Noはこの限界を表示した上で確認します。無人常駐でアカウント変更を監視し、起動中に自動書き換えする機能は含みません。

## 保存形式と検証の限界

DesktopデータフォルダーはmacOSで`~/Library/Application Support/Claude`、Windowsで`%APPDATA%\Claude`です。対象はその配下の`claude-code-sessions/<account>/<org>/local_*.json`です。保存先が異なる場合は`--data-dir`で明示します。移すのは登録情報で、同じPC上の会話本文への参照を維持します。本文そのものがなくなった会話を復旧する機能ではありません。Desktop内部形式に依存するため、アプリ更新後は`--dry-run`と画面で確認してください。件数だけで全会話が続行可能とは判断できません。

現在のアカウント候補は`config.json`の`lastKnownAccountUuid`から取得します。これは認証状態の証明ではありません。移行先組織が未作成なら、このツールでIDを推測して作成せず、Desktopにログインし直して保存状態を確認してください。

バックアップは`~/.claude-history/backups/`に保存します。元の登録には私的な情報が含まれるため、Gitや共有フォルダーへ置かないでください。ツールは認証トークンをバックアップしません。

書き込み前に復旧用の記録を保存し、処理の途中でもClaudeの再起動を確認します。途中失敗時は今回作成した未変更の登録だけを戻します。変更された登録や削除に失敗した登録は残し、バックアップ内の`manifest.json`に状況を記録します。不完全な処理の`undo`は自動実行せず、記録の確認を促します。

## ビルド・ローカル配布

開発にはRustが必要です。配布済み実行ファイルを使う側にはRust、Python、APIキーは不要です。

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
sh scripts/package.sh
sh tests/install.sh
sh tests/remote-install.sh
```

パッケージ内の`install.sh`を実行すると、SHA-256と実行ファイルの版を検証してから`~/.local/bin/claudeHistory`にインストールします。macOSで実行した場合は現在のMac向けのバイナリです。Apple SiliconとIntelの両方に配布するには、それぞれビルドと実機検証が必要です。

```sh
sh /path/to/extracted-package/install.sh
```

Windowsでの開発・配布にはx64 MSVC Rust toolchainとVisual Studio C++ Build Toolsを使用します。MSVC版はCRTを静的リンクします。

```powershell
cargo +stable-x86_64-pc-windows-msvc fmt --check
cargo +stable-x86_64-pc-windows-msvc clippy --all-targets --locked -- -D warnings
cargo +stable-x86_64-pc-windows-msvc test --locked
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tests\install.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tests\remote-install.ps1
```

共通処理は`history.rs`と`runtime.rs`、OS依存のファイル操作は`history_platform.rs`、実行状態の取得と終了要求は`runtime/macos.rs`・`runtime/windows.rs`、標準保存先は`platform.rs`に分離しています。

Windowsの終了要求はウィンドウの正常終了メッセージを使用します。SSHなどでウィンドウにアクセスできない場合は中止するため、Desktopを手動終了してください。バックアップの権限は現在のWindowsユーザーだけに制限します。WindowsではUnixのdirectory fsyncと同じ保証を提供できず、ファイルのflushとwrite-through renameを使用し、その制約を復旧記録に残します。

署名・公証、Intel Mac実機検証は未実施です。Windows実機のfixture検証と、実際のDesktop画面での会話続行は区別しています。確認済みの範囲は[VALIDATION.md](VALIDATION.md)、要件は[SPEC.md](SPEC.md)を参照してください。
