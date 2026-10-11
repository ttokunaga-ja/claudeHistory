mod uninstall;
mod update;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use claude_history::{
    history::{Account, Store},
    runtime::{self, Activity},
};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "claudeHistory",
    version,
    about = "Claude Desktop Codeのローカル履歴を別アカウントへ引き継ぐ"
)]
struct Cli {
    /// Claude Desktopのデータフォルダー（既定: macOS / Windows標準）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Claude CLIの設定フォルダー（既定: CLAUDE_CONFIG_DIR または ~/.claude）
    #[arg(long, global = true)]
    cli_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 最新の公開リリースへCLI本体を更新
    Update,
    /// 確認後にCLI本体・専用設定・バックアップを削除（Claudeの履歴は保持）
    Uninstall,
    /// 保存済みの全アカウント・組織へ不足履歴を同期
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    /// 保存済みアカウント・組織と履歴件数を表示
    Accounts,
    /// 起動・作業状態を確認（変更なし）
    Status,
    /// 現在のアカウントに不足している履歴を追加
    Transfer {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        from_org: Option<String>,
        #[arg(long)]
        to_org: Option<String>,
        /// 対象の確認のみ。履歴変更・アプリ終了はしない
        #[arg(long)]
        dry_run: bool,
    },
    /// 指定した引き継ぎで追加した登録だけを取り消す
    Undo { backup: PathBuf },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("中止: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        bail!("この版はmacOSとWindowsに対応しています。");
    }
    match &cli.command {
        Some(Command::Update) => return update::run(),
        Some(Command::Uninstall) => return uninstall::run(),
        _ => {}
    }
    let home = dirs::home_dir().context("ホームフォルダーを取得できません")?;
    let data_root = cli
        .data_dir
        .map(Ok)
        .unwrap_or_else(|| claude_history::platform::default_data_root(&home))?;
    let cli_root = cli
        .cli_dir
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home.join(".claude"));
    let store = Store {
        root: data_root.clone(),
        backup_root: home.join(".claude-history/backups"),
    };
    let command = match cli.command {
        Some(command) => command,
        None => menu(&mut io::stdin().lock(), &mut io::stdout().lock())?,
    };
    match command {
        Command::Update | Command::Uninstall => {
            unreachable!("maintenance commands handled before history access")
        }
        Command::Sync { dry_run } => {
            let current = store.current_account()?;
            let all = store.accounts()?;
            let plan = store.plan_sync(&all)?;
            println!("全アカウント・組織の同期予定:");
            for target in &plan.targets {
                println!(
                    "  {} / 追加={}件 / 既存={}件（保持）",
                    account_label(&target.account, Some(&current)),
                    target.additions,
                    target.existing
                );
            }
            if dry_run {
                println!("確認のみ: 履歴変更・アプリ終了は行いません。");
                show_activity(&runtime::inspect(&data_root, &cli_root)?);
                return Ok(());
            }
            if plan.targets.iter().all(|target| target.additions == 0) {
                println!("追加する履歴はありません。");
                return Ok(());
            }
            prepare(&data_root, &cli_root)?;
            if store.current_account()? != current {
                bail!("現在のアカウントが変わりました。もう一度実行してください。");
            }
            runtime::require_stopped()?;
            let backup = store.apply_sync_checked(&plan, runtime::require_stopped)?;
            println!("同期しました。バックアップ: {}", backup.display());
            println!(
                "Claudeを起動し、一覧と会話本文を確認してください。全会話の続行を保証するものではありません。"
            );
        }
        Command::Accounts => {
            let current = store.current_account()?;
            for account in store.accounts()? {
                println!("{}", account_label(&account, Some(&current)));
            }
            println!(
                "現在候補はDesktopの保存設定から取得しています。切り替え直後はログイン画面と照合してください。"
            );
        }
        Command::Status => show_activity(&runtime::inspect(&data_root, &cli_root)?),
        Command::Transfer {
            from,
            from_org,
            to_org,
            dry_run,
        } => {
            let current = store.current_account()?;
            let all = store.accounts()?;
            let sources: Vec<_> = all
                .iter()
                .filter(|a| {
                    a.account != current
                        && a.sessions > 0
                        && from.as_ref().is_none_or(|id| &a.account == id)
                        && from_org.as_ref().is_none_or(|id| &a.org == id)
                })
                .cloned()
                .collect();
            let source = choose("移行元", &sources, &current)?;
            let targets: Vec<_> = all
                .iter()
                .filter(|a| a.account == current && to_org.as_ref().is_none_or(|id| &a.org == id))
                .cloned()
                .collect();
            // 組織は件数から推測しない。複数あれば明示選択する。
            let target = choose("移行先（現在のアカウント）", &targets, &current)?;
            let plan = store.plan(&source, &target)?;
            println!(
                "移行元 {}\n移行先 {}\n追加={}件 / 既存={}件（保持）",
                account_label(&source, Some(&current)),
                account_label(&target, Some(&current)),
                plan.additions.len(),
                plan.existing
            );
            if dry_run {
                println!("確認のみ: 履歴変更・アプリ終了は行いません。");
                show_activity(&runtime::inspect(&data_root, &cli_root)?);
                return Ok(());
            }
            if plan.additions.is_empty() {
                println!("追加する履歴はありません。");
                return Ok(());
            }
            prepare(&data_root, &cli_root)?;
            // 終了操作中のアカウント切り替えも検出する。
            if store.current_account()? != current {
                bail!("現在のアカウントが変わりました。もう一度実行してください。");
            }
            runtime::require_stopped()?;
            let backup = store.apply_checked(&plan, runtime::require_stopped)?;
            println!(
                "{}件を追加しました。バックアップ: {}",
                plan.additions.len(),
                backup.display()
            );
            println!(
                "Claudeを起動し、一覧と会話本文を確認してください。全会話の続行を保証するものではありません。"
            );
        }
        Command::Undo { backup } => {
            prepare(&data_root, &cli_root)?;
            runtime::require_stopped()?;
            let count = store.undo_checked(&backup, runtime::require_stopped)?;
            println!(
                "今回追加した登録{count}件を取り消しました。元の会話とバックアップは保持しています。"
            );
        }
    }
    Ok(())
}

fn menu(input: &mut dyn io::BufRead, output: &mut dyn Write) -> Result<Command> {
    writeln!(
        output,
        "1: すべてのアカウントに不足履歴を同期\n2: 現在のアカウントへ引き継ぎ\n3: 状態確認"
    )?;
    match read_choice(input, output, 3)? {
        1 => Ok(Command::Sync { dry_run: false }),
        2 => Ok(Command::Transfer {
            from: None,
            from_org: None,
            to_org: None,
            dry_run: false,
        }),
        3 => Ok(Command::Status),
        _ => unreachable!(),
    }
}
fn read_choice(input: &mut dyn io::BufRead, output: &mut dyn Write, count: usize) -> Result<usize> {
    write!(output, "番号（Enterで中止）: ")?;
    output.flush()?;
    let mut text = String::new();
    if input.read_line(&mut text)? == 0 || text.trim().is_empty() {
        bail!("選択を中止しました");
    }
    let number = text
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0 && *n <= count)
        .context("候補の番号が無効です")?;
    Ok(number)
}
fn account_label(account: &Account, current: Option<&str>) -> String {
    format!(
        "account={} org={} / 履歴={}件{}",
        account.account,
        account.org,
        account.sessions,
        if current == Some(account.account.as_str()) {
            " [Desktop保存情報の現在候補]"
        } else {
            ""
        }
    )
}
fn choose(label: &str, options: &[Account], current: &str) -> Result<Account> {
    match options {
        [] => bail!("{label}の候補がありません。accountsで保存状態を確認してください。"),
        [only] => {
            println!("{label}: {}", account_label(only, Some(current)));
            Ok(only.clone())
        }
        _ => {
            println!("{label}を選んでください:");
            for (i, account) in options.iter().enumerate() {
                println!("  {}: {}", i + 1, account_label(account, Some(current)));
            }
            if !io::stdin().is_terminal() {
                bail!("候補が複数あります。--from / --from-org / --to-orgで指定してください。");
            }
            let index = read_choice(
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
                options.len(),
            )?;
            Ok(options[index - 1].clone())
        }
    }
}

fn prepare(data_root: &std::path::Path, cli_root: &std::path::Path) -> Result<()> {
    let activity = runtime::inspect(data_root, cli_root)?;
    let plan = runtime::shutdown_plan(&activity)?;
    prepare_observed(
        activity,
        &plan.pids(),
        io::stdin().is_terminal(),
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        |_| runtime::close_confirmed(&plan, data_root, cli_root),
    )
}

fn prepare_observed(
    activity: Activity,
    pids: &[i32],
    interactive: bool,
    input: &mut dyn io::BufRead,
    output: &mut dyn Write,
    close: impl FnOnce(&[i32]) -> Result<()>,
) -> Result<()> {
    if activity == Activity::Stopped {
        return Ok(());
    }
    if let Activity::Busy { reasons } | Activity::Unknown { reasons } = activity {
        for reason in reasons {
            writeln!(output, "  {reason}")?;
        }
    }
    if pids.is_empty() {
        bail!("Claudeの終了対象を安全に識別できません。手動で終了してから再実行してください。");
    }
    writeln!(
        output,
        "終了対象のClaude Desktop・CLI・ヘルパーと全子プロセス: PID {pids:?}"
    )?;
    if !interactive {
        bail!("非対話実行ではClaudeを終了しません。ターミナルから再実行してください。");
    }
    writeln!(
        output,
        "作業中の処理や未保存の状態を失う可能性があります。正常終了を要求し、5秒後の残存対象は強制終了します。"
    )?;
    write!(
        output,
        "対象のClaudeプロセスをすべて終了して続行しますか？ [Yes/No]（既定: No）: "
    )?;
    output.flush()?;
    let mut reply = String::new();
    input.read_line(&mut reply)?;
    if !matches!(reply.trim().to_ascii_lowercase().as_str(), "yes" | "y") {
        bail!("終了を許可しなかったため、プロセスと履歴は変更していません。");
    }
    close(pids)
}

fn show_activity(activity: &Activity) {
    match activity {
        Activity::Stopped => println!("Claude Desktop・CLI: 終了済み"),
        Activity::IdleDesktop { .. } => println!(
            "Claude Desktop: 起動中 / 観測中の作業兆候なし。引き継ぎ時に全対象の終了を確認します。"
        ),
        Activity::Busy { reasons } | Activity::Unknown { reasons } => {
            println!(
                "{}",
                if matches!(activity, Activity::Busy { .. }) {
                    "Claude関連プロセスまたは履歴更新を検知しました。引き継ぎ時に識別できた全対象の終了を確認します。"
                } else {
                    "作業中か判定できません。終了対象を識別できない場合は引き継ぎを中止します。"
                }
            );
            for reason in reasons {
                println!("  {reason}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io::Cursor};

    #[test]
    fn menu_one_is_all_account_sync_and_empty_input_aborts_cleanly() {
        assert!(matches!(
            menu(&mut Cursor::new("1\n"), &mut Vec::new()).unwrap(),
            Command::Sync { dry_run: false }
        ));
        for reply in ["", "\n"] {
            let error = menu(&mut Cursor::new(reply), &mut Vec::new())
                .err()
                .unwrap();
            assert_eq!(format!("{error:#}"), "選択を中止しました");
        }
    }
    #[test]
    fn sync_parses_without_registration_arguments() {
        let cli = Cli::try_parse_from(["claudeHistory", "sync", "--dry-run"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Sync { dry_run: true })));
        assert!(Cli::try_parse_from(["claudeHistory", "configure"]).is_err());
        assert!(
            Cli::try_parse_from(["claudeHistory", "--profiles-file", "accounts.json"]).is_err()
        );
        assert!(matches!(
            menu(&mut Cursor::new("3\n"), &mut Vec::new()).unwrap(),
            Command::Status
        ));
    }

    #[test]
    fn stopped_does_not_prompt_or_close() {
        let mut output = Vec::new();
        prepare_observed(
            Activity::Stopped,
            &[],
            false,
            &mut Cursor::new(b""),
            &mut output,
            |_| panic!("must not close"),
        )
        .unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn blockers_require_tty_and_explicit_yes() {
        for activity in [
            Activity::Busy {
                reasons: vec!["CLI".into()],
            },
            Activity::IdleDesktop { pids: vec![42] },
        ] {
            for reply in ["No\n", "\n", "", "anything\n"] {
                assert!(
                    prepare_observed(
                        activity.clone(),
                        &[42],
                        true,
                        &mut Cursor::new(reply),
                        &mut Vec::new(),
                        |_| panic!("must not close")
                    )
                    .is_err()
                );
            }
            assert!(
                prepare_observed(
                    activity.clone(),
                    &[42],
                    false,
                    &mut Cursor::new("Yes\n"),
                    &mut Vec::new(),
                    |_| panic!("non TTY must not close")
                )
                .is_err()
            );
            let closed = Cell::new(false);
            prepare_observed(
                activity,
                &[42],
                true,
                &mut Cursor::new("Yes\n"),
                &mut Vec::new(),
                |pids| {
                    assert_eq!(pids, &[42]);
                    closed.set(true);
                    Ok(())
                },
            )
            .unwrap();
            assert!(closed.get());
        }
    }
    #[test]
    fn unidentified_activity_and_failed_recheck_do_not_proceed() {
        assert!(
            prepare_observed(
                Activity::Unknown {
                    reasons: vec!["failed observation".into()]
                },
                &[],
                true,
                &mut Cursor::new("Yes\n"),
                &mut Vec::new(),
                |_| panic!("must not close")
            )
            .is_err()
        );
        assert!(
            prepare_observed(
                Activity::Busy {
                    reasons: vec!["CLI".into()]
                },
                &[42],
                true,
                &mut Cursor::new("Yes\n"),
                &mut Vec::new(),
                |_| bail!("identity changed")
            )
            .is_err()
        );
    }
}
