use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::signal;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Host to connect to
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,

    /// Port to connect to
    #[arg(short, long, default_value_t = 9000)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);

    println!("Connecting to {}...", addr);
    let stream = TcpStream::connect(&addr)
        .await
        .context("Failed to connect to server")?;
    println!("Connected to server!");

    let (reader, mut writer) = stream.into_split();

    // Task for reading from server and printing to stdout
    let mut reader = BufReader::new(reader);
    let read_handle = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    println!("\nServer closed connection.");
                    break;
                }
                Ok(n) => {
                    let s = String::from_utf8_lossy(&buffer[..n]);
                    print!("{}", s);
                    // Flush stdout to ensure immediate display
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                Err(e) => {
                    eprintln!("Error reading from server: {}", e);
                    break;
                }
            }
        }
    });

    // Task for reading from stdin and writing to server
    let write_handle = tokio::spawn(async move {
        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();
        
        loop {
            line.clear();
            match tokio::io::AsyncBufReadExt::read_line(&mut stdin, &mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if let Err(e) = writer.write_all(line.as_bytes()).await {
                        eprintln!("Error writing to server: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error reading from stdin: {}", e);
                    break;
                }
            }
        }
    });

    // Wait for either task to finish or Ctrl+C
    tokio::select! {
        _ = read_handle => {},
        _ = write_handle => {},
        _ = signal::ctrl_c() => {
            println!("\nDisconnecting...");
        }
    }

    Ok(())
}
