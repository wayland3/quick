// 内置命令定义
//
// 启动器输入 `> 命令名` 时触发系统级操作。

use serde::{Deserialize, Serialize};

/// 内置命令枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinCommand {
    /// 锁屏
    LockScreen,
    /// 睡眠
    Sleep,
    /// 打开设置
    OpenSettings,
    /// 清空剪贴板
    ClearClipboard,
    /// 重建索引
    RebuildIndex,
    /// 退出程序
    Quit,
}

impl BuiltinCommand {
    /// 命令名（用于匹配）
    pub fn name(&self) -> &'static str {
        match self {
            Self::LockScreen => "锁屏",
            Self::Sleep => "睡眠",
            Self::OpenSettings => "设置",
            Self::ClearClipboard => "清空剪贴板",
            Self::RebuildIndex => "重建索引",
            Self::Quit => "退出",
        }
    }

    /// 命令描述（用于显示）
    pub fn description(&self) -> &'static str {
        match self {
            Self::LockScreen => "锁定屏幕",
            Self::Sleep => "使电脑进入睡眠状态",
            Self::OpenSettings => "打开设置窗口",
            Self::ClearClipboard => "清空所有剪贴板历史",
            Self::RebuildIndex => "重新扫描并建立应用索引",
            Self::Quit => "退出 Quick",
        }
    }

    /// 从用户输入解析命令
    pub fn from_input(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('>') {
            return None;
        }
        let cmd = trimmed[1..].trim();
        match cmd {
            "锁屏" | "lock" => Some(Self::LockScreen),
            "睡眠" | "sleep" => Some(Self::Sleep),
            "设置" | "settings" => Some(Self::OpenSettings),
            "清空剪贴板" | "clear" => Some(Self::ClearClipboard),
            "重建索引" | "rebuild" => Some(Self::RebuildIndex),
            "退出" | "quit" | "exit" => Some(Self::Quit),
            _ => None,
        }
    }

    /// 所有命令列表（用于搜索）
    pub fn all() -> &'static [Self] {
        &[
            Self::LockScreen,
            Self::Sleep,
            Self::OpenSettings,
            Self::ClearClipboard,
            Self::RebuildIndex,
            Self::Quit,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析锁屏命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 锁屏"),
            Some(BuiltinCommand::LockScreen)
        );
        assert_eq!(
            BuiltinCommand::from_input("> lock"),
            Some(BuiltinCommand::LockScreen)
        );
    }

    #[test]
    fn 解析睡眠命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 睡眠"),
            Some(BuiltinCommand::Sleep)
        );
        assert_eq!(
            BuiltinCommand::from_input("> sleep"),
            Some(BuiltinCommand::Sleep)
        );
    }

    #[test]
    fn 解析设置命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 设置"),
            Some(BuiltinCommand::OpenSettings)
        );
        assert_eq!(
            BuiltinCommand::from_input("> settings"),
            Some(BuiltinCommand::OpenSettings)
        );
    }

    #[test]
    fn 解析清空剪贴板命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 清空剪贴板"),
            Some(BuiltinCommand::ClearClipboard)
        );
        assert_eq!(
            BuiltinCommand::from_input("> clear"),
            Some(BuiltinCommand::ClearClipboard)
        );
    }

    #[test]
    fn 解析重建索引命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 重建索引"),
            Some(BuiltinCommand::RebuildIndex)
        );
        assert_eq!(
            BuiltinCommand::from_input("> rebuild"),
            Some(BuiltinCommand::RebuildIndex)
        );
    }

    #[test]
    fn 解析退出命令() {
        assert_eq!(
            BuiltinCommand::from_input("> 退出"),
            Some(BuiltinCommand::Quit)
        );
        assert_eq!(
            BuiltinCommand::from_input("> quit"),
            Some(BuiltinCommand::Quit)
        );
        assert_eq!(
            BuiltinCommand::from_input("> exit"),
            Some(BuiltinCommand::Quit)
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn 非命令输入返回None() {
        assert_eq!(BuiltinCommand::from_input("notepad"), None);
        assert_eq!(BuiltinCommand::from_input("> unknown"), None);
        assert_eq!(BuiltinCommand::from_input(""), None);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 命令名称不重复() {
        let names: Vec<_> = BuiltinCommand::all().iter().map(|c| c.name()).collect();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(names.len(), unique.len());
    }
}
