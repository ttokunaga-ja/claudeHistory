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

`--version`で`claudeHistory 0.5.4`と表示されれば、`waiting-input`対応とChrome連携プロセスの誤検出修正を含む版です。旧版からは`claudeHistory update`で更新できます。403エラーで更新できない旧版は、上のインストールコマンドを再実行してください。

チェックサムや実行ファイルの検証に失敗した場合は、既存の実行ファイルを置き換えません。更新時も同じインストールコマンドを使います。配布パッケージを手動で取得する場合は[Releases](https://github.com/ttokunaga-ja/claudeHistory/releases/latest)からダウンロードし、展開したフォルダー内の`install.sh`または`install.ps1`を実行してください。展開コマンドはダウンロード先のフォルダーで実行する必要があります。

## 使い方

初回の登録や設定は不要です。以下は両OS共通です。

通常は次のコマンドでメニューを開きます。

```sh
claudeHistory
```

```text
1: すべてのアカウントに不足履歴を同期
2: 現在のアカウントへ引き継ぎ
3: 状態確認
```

**1番を選ぶと、検出したすべてのアカウント・組織へ不足分を追加します。** 同じアカウントの複数組織や履歴0件の組織も含め、3つ以上も同じ操作です。アカウント・組織のUUIDと追加予定件数、Desktopの現在のアカウント候補を表示します。メールアドレスの入力、同期対象の登録や選択はありません。起動中のClaude全対象を終了するYes/No確認を行います。

コマンドを直接指定して確認・同期することもできます。

```sh
claudeHistory accounts
claudeHistory status
claudeHistory sync --dry-run
claudeHistory sync
```

`--dry-run`は履歴を変更せず、アプリ終了も行いません。

同じIDの既存履歴は上書き・削除せず、不足分だけを追加します。同じIDで会話参照や作業フォルダーが異なる場合は、書き込み前に全体を中止します。古いタイトル等の表示情報を上書きして揃える機能ではありません。元の登録とバックアップを保持し、新しい登録の実行許可は手動にします。MCP接続、旧アカウントの許可、remote control、認証、定期実行設定は引き継ぎません。

2番、または`transfer`では従来の片方向引き継ぎも利用できます。

```sh
claudeHistory transfer --dry-run
claudeHistory transfer --from ACCOUNT_UUID --from-org SOURCE_ORG_UUID --to-org TARGET_ORG_UUID
claudeHistory undo /absolute/path/to/backup
```

`undo`は指定の実行で追加した登録だけを全対象から取り消します。追加後に変更された登録があれば、削除を始める前に中止します。会話本文と元の履歴は削除しません。

## 作業中の扱い

macOSのChrome連携用`chrome-native-host`は、所定のClaudeアプリ内のパス、Google Chromeの直接の子であること、Anthropicのコード署名、プロセスの生成識別子を確認できた場合に限り、Code履歴の同期を妨げる不明プロセスから除外します。このブラウザー連携の検出を理由にChrome本体や連携プロセスを終了対象へ追加しません。Codeの終了対象ツリーに検証済みの連携プロセスが含まれる場合も、終了要求前に中止します。署名・所有関係を確認できない同名プロセスやClaude Codeの実行は引き続き中止・終了確認の対象です。

| 検出結果 | 動作 |
| --- | --- |
| Desktop・CLIとも終了済み | そのまま引き継ぎ |
| Desktop、CLI、既知のヘルパー・その子プロセスが起動中 | 全対象のPIDを表示し、終了して続行するかYes/Noで確認 |
| No・Enter・入力終了、非対話実行 | プロセスと履歴を変更せず中止 |
| 観測失敗、未知の関連プロセス、PID再利用などの対象の置換 | 中止し、手動確認を促す |
| Claudeがこのコマンドの親・祖先 | 終了せず中止。独立したターミナルから再実行 |

**Yes/yを入力した場合だけ、Claude Desktop・CLI・ヘルパーとその全子プロセスを終了します。終了処理中に新しく起動・再起動した、識別できるClaudeも対象です。作業中の処理や未保存の状態を失う可能性があります。** 親プロセスから正常終了を要求し、5秒後に残った対象を強制終了します。新しい対象も起動時刻などを確認して追跡します。WindowsのCLI・子プロセスには一般的な正常終了手段がないため、5秒待機後に強制終了します。さらに5秒以内の終了と履歴の安定を確認してから引き継ぎます。対象が増えても待機期限は延長しません。Claudeと識別できない関連プロセスや対象の置換を検出すると中止します。

macOSの`disclaimer`ヘルパーや、Claudeが起動した拡張機能のPythonなども確認対象に含めます。確認済みの子プロセスは、親が先に終了した場合も同じプロセスであることを再確認して終了します。パスに「Claude」が含まれるだけで終了対象にすることはありません。

TCP接続や履歴更新はstatusで表示します。通信内容は読みません。対話待ちや作業中のCLIも上の確認対象です。非TTY・定期実行ではYesをパイプしても終了しません。短い観測だけで無活動を完全に保証することはできないため、書き込み直前・途中にも終了状態を確認します。

## 保存形式と検証の限界

DesktopデータフォルダーはmacOSで`~/Library/Application Support/Claude`、Windowsで`%APPDATA%\Claude`です。対象はその配下の`claude-code-sessions/<account>/<org>/local_*.json`です。同じ階層の`waiting-input`ディレクトリーはClaude本体の入力待ちデータとして、削除・読み取り・コピーをせず同期対象から除外します。登録JSONのファイル名・形式・参照先の検証と、シンボリックリンク・Windowsのジャンクション等の拒否は維持します。その他の想定外のディレクトリーは引き続き中止します。保存先が異なる場合は`--data-dir`で明示します。移すのは登録情報で、同じPC上の会話本文への参照を維持します。本文そのものがなくなった会話を復旧する機能ではありません。Desktop内部形式に依存するため、アプリ更新後は`--dry-run`と画面で確認してください。件数だけで全会話が続行可能とは判断できません。

現在のアカウント候補は`config.json`の`lastKnownAccountUuid`から取得します。これは認証状態の証明ではありません。移行先組織が未作成なら、このツールでIDを推測して作成せず、Desktopにログインし直して保存状態を確認してください。

バックアップは`~/.claude-history/backups/`に保存します。元の登録には私的な情報が含まれるため、Gitや共有フォルダーへ置かないでください。ツールは認証トークンをバックアップしません。

書き込み前に復旧用の記録を保存し、処理の途中でもClaudeの再起動を確認します。途中失敗時は今回作成した未変更の登録だけを戻します。変更された登録や削除に失敗した登録は残し、バックアップ内の`manifest.json`に状況を記録します。不完全な処理の`undo`は自動実行せず、記録の確認を促します。v0.3.0より前に作成したバックアップを取り消す場合は、作成時の版の実行ファイルを使用してください。

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

WindowsのDesktop終了要求はウィンドウの正常終了メッセージを使用し、確認後の残存対象には強制終了を使用します。バックアップの権限は現在のWindowsユーザーだけに制限します。WindowsではUnixのdirectory fsyncと同じ保証を提供できず、ファイルのflushとwrite-through renameを使用し、その制約を復旧記録に残します。

署名・公証、Intel Mac実機検証は未実施です。Windows実機のfixture検証と、実際のDesktop画面での会話続行は区別しています。確認済みの範囲は[VALIDATION.md](VALIDATION.md)、要件は[SPEC.md](SPEC.md)を参照してください。

## CLIの更新・アンインストール

最新の公開リリースへ更新するには、次のコマンドを使います。

```sh
claudeHistory update
```

GitHub APIやトークンを使わず、公開リリースページから最新版を判定します。macOS Apple Silicon / Windows x64に対応し、パッケージのSHA-256と実行ファイルの版を検証してから置き換えます。最新版ならそのまま終了します。通常の同期や`--version`では更新確認・通信を行いません。更新時はインターネット接続、OS標準のcurl、実行ファイルのフォルダーへの書き込み権限が必要です。v0.3.0以前から更新する場合は、最初の1回だけ冒頭のインストールコマンドを再実行してください。

更新時に403エラーが出る旧版では、冒頭のインストールコマンドを再実行してください。履歴とバックアップは変更しません。

このCLIと専用の保存データを削除するには、次のコマンドを使います。

```sh
claudeHistory uninstall
```

削除する実行ファイルの絶対パスを表示し、`[y/N]`で確認します。`y`・`yes`・`はい`のみ削除を許可し、Enter・EOF・その他は中止します。**削除対象はclaudeHistory本体と`~/.claude-history`です。専用フォルダーに残っているファイルと復旧用バックアップも削除します。Claudeの履歴・設定・認証情報、PATH・共有binフォルダーは残ります。**

Windowsでは終了後に本体を削除するため、結果のJSON記録を表示します。`deleted`なら削除完了、`scheduled`なら待機中、`failed`なら失敗です。元のパスに後から置かれた別の実行ファイルは削除しません。失敗時は表示された記録と退避ファイルを確認してください。
