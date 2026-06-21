//! crossterm 键盘输入 → mpsc 桥接。
//!
//! `crossterm::event::read()` 是阻塞同步调用，必须在 `spawn_blocking` 中
//! 运行。用 mpsc 通道将按键事件传递到渲染循环。

use crossterm::event::{self, Event, KeyEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 键盘输入消息。
pub enum InputMsg {
    Key(KeyEvent),
    Resize(u16, u16),
}

/// 启动输入读取 task：crossterm 同步读取 → mpsc。
pub fn spawn_input_reader(mpsc_tx: mpsc::Sender<InputMsg>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || loop {
        match event::read() {
            Ok(Event::Key(key)) => {
                if mpsc_tx.blocking_send(InputMsg::Key(key)).is_err() {
                    break;
                }
            }
            Ok(Event::Resize(w, h)) => {
                if mpsc_tx.blocking_send(InputMsg::Resize(w, h)).is_err() {
                    break;
                }
            }
            _ => {}
        }
    })
}
