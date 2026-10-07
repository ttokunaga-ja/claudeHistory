# 初版の検証

- Rust fixtureテスト22件成功。重複回避、既存履歴保持、競合、権限除去、バックアップ、途中失敗の復旧記録、undoの変更検出、シンボリックリンク、排他、作業の再開、Yes/No/EOF、TCP観測、Node argvの空白を含むパスを検証。
- `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`cargo build --release --locked`成功。
- `sh tests/install.sh`で正常インストール、破損チェックサム、重複チェックサムの拒否と既存バイナリ保持を検証。インストール先は破棄可能なfixtureフォルダー。
- このMacの実行ファイルで`status`と`transfer --dry-run`を実行。実際のClaude CLIとDesktop子プロセスを検知し、作業中メッセージを確認。実データの書き込みとプロセス終了は実行していない。
- 実際のDesktopをYes選択で終了する操作と、生成した初版による実会話の続行は未検証。過去の手動復旧とfixture成功を初版の実機受入と同一視しない。
- 配布物はmacOS Apple Silicon向けのローカルパッケージ。Intel実機、Windows、署名・公証、GitHub公開Releaseは未実施。

機械的な検出に完全性はない。CLI対話待ち、Desktopの待機TCP接続も保守的に中止する。安全な終了を判定できない場合は、ユーザーが作業を終え、Desktop・CLIを手動終了してから実行する。
