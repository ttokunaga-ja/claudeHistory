# v0.2.0の検証

2026-10-07にmacOS Apple Siliconと、SSH接続したWindows x64実機で検証。WindowsはRust 1.97.1 / x86_64-pc-windows-msvcとVisual Studio C++ Build Toolsを使用。

- macOS 30件、Windows 33件のRustテストが成功。
- 両OSで`cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`cargo build --release --locked`を実行。WindowsではMSVC toolchainを明示する。
- 共通fixtureで不足分追加、重複回避、既存保持、競合、権限除去、バックアップ、途中失敗と復旧記録、undoの変更検出、作業再開時の中止、別プロセスとの排他を検証。
- Windows固有のfixtureでジャンクション・祖先reparse point拒否、バックアップの現在ユーザーSIDだけのDACL、実際のCIMプロセス取得・TCPテーブル取得を検証。
- Windowsの正常終了要求は、所有するテスト用ウィンドウにWM_CLOSEを送り通常のメッセージ処理で閉じることを検証。別PIDのウィンドウや操作できないウィンドウは拒否。SSHセッションで通常のDesktopウィンドウが取得できることの証明ではない。
- 両OSのインストーラーを破棄可能なフォルダーで実行。正常インストール、既存版の置換、破損・重複チェックサムの拒否と既存実行ファイル保持を確認。Windowsでは正しいチェックサムでも版が異なる実行ファイルを拒否することも確認。

## 実データとの境界

このMacでは実際のClaude CLI・Desktop作業用子プロセスをread-onlyで検知する。実データへの書き込みや実アプリの終了は実行しない。

Windows検証機の標準保存先にはClaude Desktopの登録データがない。MSIX版Claudeの常駐`cowork-svc.exe`は起動中で、実行ファイルの`status`は安全に作業を判定できないものとして中止を表示する。この実サービスは停止しない。独立したfixture登録で配布実行ファイルの`accounts`・`status`・`transfer --dry-run`を確認し、独立した模擬CLIプロセスの起動中に引き継ぎが中止されることを確認する。引き継ぎ・undoのファイル操作はネイティブの共通fixtureテストで検証する。実際のDesktopの画面で全会話を続行する受入は未実施。

機械的な検出に完全性はない。観測後に作業が再開する競合や、他のプログラムがファイルを差し替える競合は残る。CLI対話待ち、Desktopの待機TCP接続も保守的に中止する。判定できない場合は作業を終え、Desktop・CLIを手動終了してから実行する。

配布はmacOS Apple SiliconとWindows x64。Intel Mac実機、署名・公証は未実施。GitHub Actionsは使わず、両実機で検証したパッケージとSHA256SUMSをReleaseへアップロードする。

## 自動ダウンロード型インストーラー

ルートの`install.sh` / `install.ps1`を追加。公開済みv0.2.0のパッケージを取得するため、実行ファイルとReleaseの変更は不要。macOSはダウンロード失敗、外側・内側のチェックサム不一致、重複項目の拒否と既存版保持、PATHの引用と設定の重複回避を隔離HOMEで検証した。Windowsはネイティブ実機で正常インストール・置換、不正・重複・破損したチェックサム、不正なインストール先、同梱インストーラーの失敗を検証した。Windowsの検証は`CLAUDE_HISTORY_INSTALL_NO_PATH=1`でユーザーのレジストリPATHを変更せず、変更されていないことを確認した。WindowsのPATH設定コードはaiUsageと同じ方式で、現在のユーザー設定を書き換える実機テストは行わない。
