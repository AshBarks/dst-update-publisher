use clap::Parser;

#[derive(Parser)]
#[command(name = "dst-update-publisher", about = "DST 更新公告翻译推送")]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(
        long,
        short = 'i',
        help = "轮询间隔（秒），设置后启用轮询模式；不设置则单次执行后退出"
    )]
    pub poll_interval: Option<u64>,
}

#[derive(Debug, Clone, Parser)]
pub enum Commands {
    #[command(
        name = "translate-json",
        about = "翻译 JSON 文件中的 content_text 字段"
    )]
    TranslateJson {
        #[arg(short, long, help = "输入需要翻译的 JSON 文件路径")]
        input: String,
        #[arg(short, long, help = "翻译结果输出 JSON 文件路径")]
        output: Option<String>,
        #[arg(short, long, default_value = "4", help = "并发翻译最大数")]
        concurrency: usize,
    },
}

#[derive(Debug, Clone)]
pub enum RunMode {
    Once,
    Poll {
        interval_secs: u64,
    },
    TranslateJson {
        input: String,
        output: String,
        concurrency: usize,
    },
}

impl CliArgs {
    pub fn run_mode(&self) -> RunMode {
        if let Some(cmd) = &self.command {
            match cmd {
                Commands::TranslateJson {
                    input,
                    output,
                    concurrency,
                } => {
                    let out = output.clone().unwrap_or_else(|| {
                        let stem = input.trim_end_matches(".json");
                        format!("{}_translated.json", stem)
                    });
                    RunMode::TranslateJson {
                        input: input.clone(),
                        output: out,
                        concurrency: *concurrency,
                    }
                }
            }
        } else {
            match self.poll_interval {
                Some(secs) if secs > 0 => RunMode::Poll {
                    interval_secs: secs,
                },
                _ => RunMode::Once,
            }
        }
    }
}
