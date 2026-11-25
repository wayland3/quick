pub mod appindex;
pub mod clipboard;
pub mod commands;
pub mod hotkey;
pub mod jsondetail;
pub mod logging;
pub mod notify;
pub mod paste;
pub mod query;
pub mod ranking;
pub mod retention;
pub mod selection;
pub mod storage;
pub mod thumbnail;
pub mod translate;
pub mod tray;
pub mod window;

// 业务逻辑层：storage、search、ranking、hotkey、config、tray、window
// 不直接调用 Win32，通过 platform 模块
