//! Minimal native client: subscribe, set ERG, inject a shift, then print events.
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:9376/ws".into());
    let (mut socket, _) = connect_async(&url)
        .await
        .context("Start `cargo run -p bikebridge-cli -- run --mock` first")?;
    for command in [
        json!({"type":"subscribe","requestId":"subscribe","events":["telemetry","input","device"]}),
        json!({"type":"trainer.setTargetPower","requestId":"erg","deviceId":"mock-trainer","data":{"watts":250}}),
        json!({"type":"mock.input","requestId":"shift","deviceId":"mock-controller","data":{"input":"shift_up","state":"pressed"}}),
    ] {
        socket
            .send(Message::Text(command.to_string().into()))
            .await?;
    }
    println!("Connected to {url}. Ctrl+C to disconnect and release trainer control.");
    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; socket.close(None).await?; break; }
            message = socket.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let value: Value = serde_json::from_str(&text)?;
                        println!("{value}");
                        if value["type"] == "response" && value["success"] == false { bail!("Command rejected: {}", value["error"]); }
                    }
                    Some(Ok(Message::Ping(data))) => socket.send(Message::Pong(data)).await?,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => {},
                }
            }
        }
    }
    Ok(())
}
