use clap::Args;

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Run the scheduler as a detached background daemon (Unix only).
    #[arg(long, short = 'd', conflicts_with_all = ["stop", "install", "uninstall", "status"])]
    pub detach: bool,

    /// Stop a running daemon (reads `.omakure/daemon.pid` and sends SIGTERM).
    #[arg(long, conflicts_with_all = ["install", "uninstall", "status"])]
    pub stop: bool,

    /// Install a systemd user service that runs `omakure serve` for the
    /// current workspace and survives reboots (Linux only).
    #[arg(long, conflicts_with_all = ["uninstall", "status"])]
    pub install: bool,

    /// Disable and remove the systemd user service for the current
    /// workspace (Linux only).
    #[arg(long, conflicts_with_all = ["status"])]
    pub uninstall: bool,

    /// Print the systemd user service status for the current workspace
    /// (Linux only).
    #[arg(long)]
    pub status: bool,

    /// Do not spawn the in-process worker. Use when you already run
    /// `omakure queue worker` elsewhere.
    #[arg(long = "no-worker")]
    pub no_worker: bool,

    /// Number of worker threads for the in-process worker (default 1).
    #[arg(long, default_value_t = 1)]
    pub concurrency: u32,

    /// Test convenience: run a single scheduler tick, enqueue whatever is
    /// due, then exit. Hidden from --help. Used by integration tests.
    #[arg(long, hide = true)]
    pub once: bool,
}
