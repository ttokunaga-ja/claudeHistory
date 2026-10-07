use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use claude_history::{
    history::{Account, Store},
    profiles::{Profile, Profiles, validate_email, validate_label},
    runtime::{self, Activity},
};
use std::{
    collections::BTreeMap,
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
    /// メールアドレス・組織の表示設定（既定: ~/.claude-history/accounts.json）
    #[arg(long, global = true)]
    profiles_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 全登録アカウントへ不足履歴を同期
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    /// メールアドレス・組織名・同期対象組織を設定
    Configure,
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
    let profiles_path = cli
        .profiles_file
        .unwrap_or_else(|| home.join(".claude-history/accounts.json"));
    let interactive = cli.command.is_none();
    let command = match cli.command {
        Some(command) => command,
        None => menu(&mut io::stdin().lock(), &mut io::stdout().lock())?,
    };
    let mut profiles = if matches!(&command, Command::Status | Command::Undo { .. }) {
        Profiles::default()
    } else {
        Profiles::load(&profiles_path)?
    };
    match command {
        Command::Configure => {
            let all = store.accounts()?;
            let current = store.current_account()?;
            configure(
                &all,
                &mut profiles,
                Some(&current),
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
            )?;
            profiles.save(&profiles_path)?;
            println!("アカウント設定を保存しました。");
        }
        Command::Sync { dry_run } => {
            let current = store.current_account()?;
            let all = store.accounts()?;
            if interactive && !dry_run && profiles.selected(&all).is_err() {
                println!("同期対象のメールアドレス・組織を設定してください。");
                configure(
                    &all,
                    &mut profiles,
                    Some(&current),
                    &mut io::stdin().lock(),
                    &mut io::stdout().lock(),
                )?;
                profiles.save(&profiles_path)?;
            }
            let selected = profiles.selected(&all)?;
            let plan = store.plan_sync(&selected)?;
            println!("全アカウント同期の事前確認:");
            for target in &plan.targets {
                println!(
                    "  {} / 追加={}件 / 既存={}件（保持）",
                    profiles.label(&target.account, Some(&current)),
                    target.additions,
                    target.existing
                );
            }
            if dry_run {
                println!("確認のみ: 設定保存・履歴変更・アプリ終了は行いません。");
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
                println!("{}", profiles.label(&account, Some(&current)));
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
            let source = choose("移行元", &sources, &profiles, &current)?;
            let targets: Vec<_> = all
                .iter()
                .filter(|a| a.account == current && to_org.as_ref().is_none_or(|id| &a.org == id))
                .cloned()
                .collect();
            // 組織は件数から推測しない。複数あれば明示選択する。
            let target = choose("移行先（現在のアカウント）", &targets, &profiles, &current)?;
            let plan = store.plan(&source, &target)?;
            println!(
                "移行元 {}\n移行先 {}\n追加={}件 / 既存={}件（保持）",
                profiles.label(&source, Some(&current)),
                profiles.label(&target, Some(&current)),
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
        "1: すべてのアカウントに不足履歴を同期\n2: 現在のアカウントへ引き継ぎ\n3: アカウント設定\n4: 状態確認"
    )?;
    match read_choice(input, output, 4)? {
        1 => Ok(Command::Sync { dry_run: false }),
        2 => Ok(Command::Transfer {
            from: None,
            from_org: None,
            to_org: None,
            dry_run: false,
        }),
        3 => Ok(Command::Configure),
        4 => Ok(Command::Status),
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
fn read_label(
    prompt: &str,
    old: Option<&str>,
    email: bool,
    input: &mut dyn io::BufRead,
    output: &mut dyn Write,
) -> Result<String> {
    loop {
        if let Some(value) = old {
            write!(output, "{prompt}（Enterで保持: {value}）: ")?;
        } else {
            write!(output, "{prompt}（Enterで中止）: ")?;
        }
        output.flush()?;
        let mut value = String::new();
        if input.read_line(&mut value)? == 0 {
            bail!("選択を中止しました");
        }
        let value = value.trim_end_matches(['\r', '\n']);
        if value.is_empty() {
            return old.map(str::to_owned).context("選択を中止しました");
        }
        match if email {
            validate_email(value)
        } else {
            validate_label(value)
        } {
            Ok(()) => return Ok(value.to_owned()),
            Err(error) => writeln!(output, "{error}")?,
        }
    }
}
fn configure(
    all: &[Account],
    profiles: &mut Profiles,
    current: Option<&str>,
    input: &mut dyn io::BufRead,
    output: &mut dyn Write,
) -> Result<()> {
    let mut groups = BTreeMap::new();
    for account in all {
        groups
            .entry(&account.account)
            .or_insert_with(Vec::new)
            .push(account);
    }
    if groups.is_empty() {
        bail!("保存済みアカウントがありません。");
    }
    let mut updated = profiles.clone();
    for (id, organizations) in groups {
        writeln!(output, "アカウント設定:")?;
        for account in &organizations {
            writeln!(output, "  {}", profiles.label(account, current))?;
        }
        let old = profiles.accounts.get(id);
        let email = read_label(
            "メールアドレス（ユーザー登録・認証確認ではありません）",
            old.map(|p| p.email.as_str()),
            true,
            input,
            output,
        )?;
        let mut names = BTreeMap::new();
        for account in &organizations {
            writeln!(output, "  {}", profiles.label(account, current))?;
            let name = read_label(
                "組織の表示名",
                old.and_then(|p| p.organizations.get(&account.org))
                    .map(String::as_str),
                false,
                input,
                output,
            )?;
            names.insert(account.org.clone(), name);
        }
        let selected = if organizations.len() == 1 {
            organizations[0]
        } else {
            writeln!(output, "{email} の同期対象組織（1個）:")?;
            for (index, account) in organizations.iter().enumerate() {
                writeln!(
                    output,
                    "  {}: {} / {} / 履歴={}件{}",
                    index + 1,
                    email,
                    names[&account.org],
                    account.sessions,
                    if current == Some(id.as_str()) {
                        " [Desktop保存情報の現在候補]"
                    } else {
                        ""
                    }
                )?;
            }
            organizations[read_choice(input, output, organizations.len())? - 1]
        };
        updated.accounts.insert(
            id.clone(),
            Profile {
                email,
                selected_org: selected.org.clone(),
                organizations: names,
            },
        );
    }
    *profiles = updated;
    Ok(())
}
fn choose(label: &str, options: &[Account], profiles: &Profiles, current: &str) -> Result<Account> {
    match options {
        [] => bail!("{label}の候補がありません。accountsで保存状態を確認してください。"),
        [only] => {
            println!("{label}: {}", profiles.label(only, Some(current)));
            Ok(only.clone())
        }
        _ => {
            println!("{label}を選んでください:");
            for (i, account) in options.iter().enumerate() {
                println!("  {}: {}", i + 1, profiles.label(account, Some(current)));
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
    fn configure_three_accounts_and_eof_preserves_settings() {
        let all: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|id| Account {
                account: id.into(),
                org: "org".into(),
                sessions: 2,
            })
            .collect();
        let mut profiles = Profiles::default();
        let mut output = Vec::new();
        configure(
            &all,
            &mut profiles,
            Some("b"),
            &mut Cursor::new(
                "a@example.com\nPersonal\nb@example.com\nWork\nc@example.com\nResearch\n",
            ),
            &mut output,
        )
        .unwrap();
        assert_eq!(profiles.selected(&all).unwrap().len(), 3);
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Desktop保存情報の現在候補")
        );
        let before = profiles.clone();
        assert!(
            configure(
                &all,
                &mut profiles,
                None,
                &mut Cursor::new("new@example.com\nNew\n"),
                &mut Vec::new()
            )
            .is_err()
        );
        assert_eq!(before, profiles);
    }
    #[test]
    fn configure_requires_numbered_organization_selection() {
        let all = vec![
            Account {
                account: "a".into(),
                org: "first".into(),
                sessions: 1,
            },
            Account {
                account: "a".into(),
                org: "second".into(),
                sessions: 4,
            },
        ];
        let mut profiles = Profiles::default();
        let mut output = Vec::new();
        configure(
            &all,
            &mut profiles,
            None,
            &mut Cursor::new("a@example.com\nPersonal\nTeam\n2\n"),
            &mut output,
        )
        .unwrap();
        assert_eq!(profiles.accounts["a"].selected_org, "second");
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("2: a@example.com / Team / 履歴=4件")
        );
    }
    #[test]
    fn dry_run_sync_parses_and_loads_no_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("missing/accounts.json");
        let cli = Cli::try_parse_from([
            "claudeHistory",
            "--profiles-file",
            path.to_str().unwrap(),
            "sync",
            "--dry-run",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Sync { dry_run: true })));
        Profiles::load(cli.profiles_file.as_ref().unwrap()).unwrap();
        assert!(!path.parent().unwrap().exists());
    }

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
