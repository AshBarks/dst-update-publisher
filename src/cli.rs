use clap::Parser;

#[derive(Parser)]
#[command(name = "dst-update-publisher", about = "DST 更新公告翻译推送")]
pub struct CliArgs {
    #[arg(
        long,
        short = 'i',
        help = "轮询间隔（秒），设置后启用轮询模式；不设置则单次执行后退出"
    )]
    pub poll_interval: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum RunMode {
    Once,
    Poll { interval_secs: u64 },
}

impl CliArgs {
    pub fn run_mode(&self) -> RunMode {
        match self.poll_interval {
            Some(secs) if secs > 0 => RunMode::Poll {
                interval_secs: secs,
            },
            _ => RunMode::Once,
        }
    }
}
