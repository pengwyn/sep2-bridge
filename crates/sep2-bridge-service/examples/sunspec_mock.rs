#[path = "../tests/modbus_server_mock/mod.rs"]
mod modbus_server_mock;

use std::io::Write;
use std::net::SocketAddr;

use clap::Parser;
use modbus_server_mock::SunSpecMock;
use sunspec::models::model701::{ConnSt, St};
use tokio::io::{AsyncBufReadExt, BufReader};

/// Runs the SunSpec modbus mock used by the integration tests as a standalone server.
#[derive(Parser)]
struct Args {
    /// Address to listen on.
    #[clap(long, default_value = "127.0.0.1:5020")]
    addr: SocketAddr,

    /// Only expose these SunSpec models, comma separated. Defaults to all of them.
    #[clap(long, value_delimiter = ',')]
    models: Option<Vec<u32>>,
}

/// Runs a single REPL command. Returns false when the loop should exit.
fn run_command(mock: &SunSpecMock, line: &str) -> bool {
    let cmd = line.trim();
    match cmd {
        "" => {}
        "connect" => {
            mock.set_value("model701::CONN_ST", Some(ConnSt::Connected));
        }
        "disconnect" => {
            mock.set_value("model701::CONN_ST", Some(ConnSt::Disconnected));
        }
        "operation_on" => {
            mock.set_value("model701::ST", Some(St::On));
        }
        "operation_off" => {
            mock.set_value("model701::ST", Some(St::Off));
        }
        "quit" | "exit" => return false,
        other => println!("Unknown command {other:?};"),
    }
    println!("Did {cmd}");
    true
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mut mock = SunSpecMock::new(args.models.as_deref()).await?;
    mock.addr = Some(args.addr);
    mock.start().await?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("> ");
        std::io::stdout().flush()?;
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => if !run_command(&mock, &line) { break },
                None => { println!(); break }          // EOF / Ctrl-D
            },
            _ = tokio::signal::ctrl_c() => { println!(); break }
        }
    }

    mock.stop().await;
    Ok(())
}
