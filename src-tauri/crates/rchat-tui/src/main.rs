fn main() -> anyhow::Result<()> {
    let args = std::env::args_os().collect::<Vec<_>>();
    match rchat_tui::ratty_host::launch_if_needed(&args)? {
        rchat_tui::ratty_host::LaunchOutcome::RunHere { warning } => {
            if let Some(warning) = warning {
                eprintln!("{warning}");
            }
            rchat_tui::app::run()
        }
        rchat_tui::ratty_host::LaunchOutcome::HostedExited(status) if status.success() => Ok(()),
        rchat_tui::ratty_host::LaunchOutcome::HostedExited(status) => {
            Err(anyhow::anyhow!("terminal host exited with {status}"))
        }
    }
}
