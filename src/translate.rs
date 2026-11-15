//! Selection translation bridge to local Pot (`127.0.0.1:60828`).
use crate::theme::color as rgb;
use gpui_kit::*;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::Command,
    thread,
    time::Duration,
};

const POT_ADDR: ([u8; 4], u16) = ([127, 0, 0, 1], 60828);
const POT_EXE: &str = r"D:\applications\pot\pot.exe";

/// Send `text` to Pot's local HTTP API (`POST /`), auto-launching `POT_EXE` if needed.
pub fn send_to_pot(text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if post_pot_http(trimmed).is_ok() {
        return Ok(());
    }
    if Path::new(POT_EXE).is_file() {
        let _ = Command::new(POT_EXE).spawn();
        for _ in 0..14 {
            thread::sleep(Duration::from_millis(250));
            if post_pot_http(trimmed).is_ok() {
                return Ok(());
            }
        }
    }
    Err("未能连接本地 Pot（127.0.0.1:60828），请确认 Pot 已启动并开启服务端口".to_owned())
}

fn post_pot_http(text: &str) -> std::io::Result<()> {
    let addr = SocketAddr::from(POT_ADDR);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let body = text.as_bytes();
    let header = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1:60828\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf)?;
    let status_line = std::str::from_utf8(&buf[..n]).unwrap_or("");
    if status_line.starts_with("HTTP/1.") && status_line.contains(" 200") {
        Ok(())
    } else if status_line.starts_with("HTTP/1.") {
        // Any 2xx from Pot means the translation window request was accepted.
        let code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("Pot returned HTTP {code}")))
        }
    } else {
        Err(std::io::Error::other("Invalid HTTP response from Pot"))
    }
}

/// Dispatch `send_to_pot` off-thread and surface any connection error briefly on the pane.
pub fn spawn_pot_translate<T: 'static>(
    owner: &mut T,
    text: String,
    cx: &mut Context<T>,
    set_error: fn(&mut T, Option<String>),
) -> Task<()> {
    set_error(owner, None);
    let task_bg = cx.background_executor().spawn(async move { send_to_pot(&text) });
    cx.spawn(async move |this, cx| {
        if let Err(err) = task_bg.await {
            let _ = this.update(cx, |owner, cx| {
                set_error(owner, Some(err));
                cx.notify();
            });
            cx.background_executor()
                .timer(Duration::from_millis(3500))
                .await;
            let _ = this.update(cx, |owner, cx| {
                set_error(owner, None);
                cx.notify();
            });
        }
    })
}

pub fn render_error_toast(id: &'static str, message: &str) -> AnyElement {
    div()
        .absolute()
        .top_3()
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded(px(8.))
                .bg(rgb(0xfff4e5))
                .border_1()
                .border_color(rgb(0xf2c97d))
                .shadow_md()
                .text_xs()
                .text_color(rgb(0x854d0e))
                .child(message.to_owned()),
        )
        .into_any_element()
}
