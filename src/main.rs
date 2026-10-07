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
    match cli.command.unwrap_or(Command::Transfer {
        from: None,
        from_org: None,
        to_org: None,
        dry_run: false,
    }) {
        Command::Accounts => {
            let current = store.current_account()?;
            for account in store.accounts()? {
                println!(
                    "{} account={} org={} 履歴={}件",
                    if account.account == current {
                        "現在候補"
                    } else {
                        "保存済み"
                    },
                    account.account,
                    account.org,
                    account.sessions
                );
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
            let source = choose("移行元", &sources)?;
            let targets: Vec<_> = all
                .iter()
                .filter(|a| a.account == current && to_org.as_ref().is_none_or(|id| &a.org == id))
                .cloned()
                .collect();
            // 組織は件数から推測しない。複数あれば明示選択する。
            let target = choose("移行先（現在のアカウント）", &targets)?;
            let plan = store.plan(&source, &target)?;
            println!(
                "移行元 account={} org={}\n移行先 account={} org={}\n追加={}件 / 既存={}件（保持）",
                source.account,
                source.org,
                target.account,
                target.org,
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

fn choose(label: &str, options: &[Account]) -> Result<Account> {
    match options {
        [] => bail!("{label}の候補がありません。accountsで保存状態を確認してください。"),
        [only] => Ok(only.clone()),
        _ => {
            println!("{label}を選んでください:");
            for (i, a) in options.iter().enumerate() {
                println!(
                    "  {}: account={} org={} 履歴={}件",
                    i + 1,
                    a.account,
                    a.org,
                    a.sessions
                );
            }
            if !io::stdin().is_terminal() {
                bail!("候補が複数あります。--from / --from-org / --to-orgで指定してください。");
            }
            print!("番号（Enterで中止）: ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let index = input
                .trim()
                .parse::<usize>()
                .context("選択を中止しました")?;
            options
                .get(index.wrapping_sub(1))
                .cloned()
                .context("候補の番号が無効です")
        }
    }
}

fn prepare(data_root: &std::path::Path, cli_root: &std::path::Path) -> Result<()> {
    prepare_observed(
        runtime::inspect(data_root, cli_root)?,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        |pids| runtime::close_idle_desktop(pids, data_root, cli_root),
    )
}

fn prepare_observed(
    activity: Activity,
    input: &mut dyn io::BufRead,
    output: &mut dyn Write,
    close: impl FnOnce(&[i32]) -> Result<()>,
) -> Result<()> {
    match activity {
        Activity::Stopped => Ok(()),
        Activity::Busy { reasons } => {
            for reason in reasons {
                writeln!(output, "  {reason}")?;
            }
            bail!("作業中です。作業を終了してから進めてください。")
        }
        Activity::Unknown { reasons } => {
            for reason in reasons {
                writeln!(output, "  {reason}")?;
            }
            bail!(
                "作業中か安全に判定できません。Claude Desktop・CLIと関連作業を終了してから進めてください。"
            )
        }
        Activity::IdleDesktop { pids } => {
            writeln!(
                output,
                "Claude Desktopが起動しています。観測中に作業の兆候はありませんでしたが、無活動を完全に保証するものではありません。"
            )?;
            write!(
                output,
                "アプリを終了して引き継ぎを進めますか？ [Yes/No]（既定: No）: "
            )?;
            output.flush()?;
            let mut reply = String::new();
            input.read_line(&mut reply)?;
            if !matches!(reply.trim().to_ascii_lowercase().as_str(), "yes" | "y") {
                bail!("終了を許可しなかったため、履歴は変更していません。");
            }
            close(&pids)
        }
    }
}

fn show_activity(activity: &Activity) {
    match activity {
        Activity::Stopped => println!("Claude Desktop・CLI: 終了済み"),
        Activity::IdleDesktop { .. } => println!(
            "Claude Desktop: 起動中 / 観測中の作業兆候なし。引き継ぎ時に終了のYes/Noを確認します。"
        ),
        Activity::Busy { reasons } | Activity::Unknown { reasons } => {
            println!(
                "{}",
                if matches!(activity, Activity::Busy { .. }) {
                    "作業中です。作業を終了してから進めてください。"
                } else {
                    "作業中か判定できません。引き継ぎを中止します。"
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
    fn stopped_does_not_prompt_or_close() {
        let mut output = Vec::new();
        prepare_observed(
            Activity::Stopped,
            &mut Cursor::new(b""),
            &mut output,
            |_| panic!("must not close"),
        )
        .unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn busy_and_unknown_never_close_even_with_yes() {
        for activity in [
            Activity::Busy {
                reasons: vec!["CLI".into()],
            },
            Activity::Unknown {
                reasons: vec!["TCP".into()],
            },
        ] {
            assert!(
                prepare_observed(
                    activity,
                    &mut Cursor::new(b"Yes\n"),
                    &mut Vec::new(),
                    |_| panic!("must not close")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn idle_requires_explicit_yes_and_rechecks_via_closer() {
        for reply in ["No\n", "\n", "", "anything\n"] {
            assert!(
                prepare_observed(
                    Activity::IdleDesktop { pids: vec![42] },
                    &mut Cursor::new(reply),
                    &mut Vec::new(),
                    |_| panic!("must not close")
                )
                .is_err()
            );
        }
        let closed = Cell::new(false);
        prepare_observed(
            Activity::IdleDesktop { pids: vec![42] },
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
        assert!(
            prepare_observed(
                Activity::IdleDesktop { pids: vec![42] },
                &mut Cursor::new("Yes\n"),
                &mut Vec::new(),
                |_| bail!("activity changed")
            )
            .is_err()
        );
    }
}
