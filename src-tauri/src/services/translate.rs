// 翻译服务
//
// 提供本地词典查询（英→中 ECDICT、中→英 CC-CEDICT）与搜索 URL 构建。
// 懒加载：首次查询时才打开词典数据库，失败后置位避免重复尝试。

use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;
use std::sync::Mutex;

/// 翻译方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateDirection {
    EnToZh,
    ZhToEn,
}

/// 本地词典查询结果
#[derive(Debug, Clone)]
pub struct LocalTranslation {
    pub phonetic: Option<String>,
    pub definitions: String,
}

/// 翻译服务（懒加载词典）
pub struct TranslateService {
    state: Mutex<TranslateState>,
    resource_dir: PathBuf,
}

struct TranslateState {
    ecdict: Option<Connection>,
    cedict: Option<Connection>,
    failed: bool, // 打开失败后置位，本次运行不再重试
}

impl TranslateService {
    /// 创建翻译服务（不立即加载词典）
    pub fn new(resource_dir: PathBuf) -> Self {
        Self {
            state: Mutex::new(TranslateState {
                ecdict: None,
                cedict: None,
                failed: false,
            }),
            resource_dir,
        }
    }

    /// 确保词典已加载（懒加载）
    fn ensure_loaded(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());

        if state.failed {
            return Err("词典加载已失败，本次运行不再重试".to_string());
        }

        if state.ecdict.is_some() && state.cedict.is_some() {
            return Ok(());
        }

        // 尝试打开词典
        let ecdict_path = self.resource_dir.join("ecdict.db");
        let cedict_path = self.resource_dir.join("cedict.db");

        let ecdict = if ecdict_path.exists() {
            Connection::open_with_flags(
                &ecdict_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        } else {
            None
        };

        let cedict = if cedict_path.exists() {
            Connection::open_with_flags(
                &cedict_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        } else {
            None
        };

        if ecdict.is_none() || cedict.is_none() {
            state.failed = true;
            log::warn!(
                "词典文件缺失：ecdict.db={}, cedict.db={}",
                ecdict_path.exists(),
                cedict_path.exists()
            );
            return Err("词典文件缺失".to_string());
        }

        state.ecdict = ecdict;
        state.cedict = cedict;
        Ok(())
    }

    /// 查询翻译
    pub fn translate(&self, input: &str) -> Option<LocalTranslation> {
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed.len() > 100 {
            return None;
        }

        // 至少含一个拉丁字母或 CJK 字符
        let has_alpha_or_cjk = trimmed.chars().any(|c| c.is_alphabetic());
        if !has_alpha_or_cjk {
            return None;
        }

        // 确保词典已加载
        if self.ensure_loaded().is_err() {
            return None;
        }

        let direction = detect_language(trimmed);
        let normalized = normalize(trimmed);

        match direction {
            TranslateDirection::EnToZh => self.query_ecdict(&normalized),
            TranslateDirection::ZhToEn => self.query_cedict(&normalized),
        }
    }

    /// 查询 ECDICT（英→中）
    fn query_ecdict(&self, normalized: &str) -> Option<LocalTranslation> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let conn = state.ecdict.as_ref()?;

        // 精确匹配 sw 列
        let mut stmt = conn
            .prepare("SELECT phonetic, translation FROM ecdict WHERE sw = ?1 LIMIT 1")
            .ok()?;

        let result = stmt
            .query_row([normalized], |row| {
                Ok(LocalTranslation {
                    phonetic: row.get(0)?,
                    definitions: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                })
            })
            .ok();

        if result.is_some() {
            return result;
        }

        // 未命中 → 查 lemma 反向表取原型再查
        let mut stmt = conn
            .prepare("SELECT base FROM ecdict_lemma WHERE inflected = ?1 LIMIT 1")
            .ok()?;

        let base: Option<String> = stmt.query_row([normalized], |row| row.get(0)).ok();

        if let Some(base_word) = base {
            let base_normalized = normalize(&base_word);
            let mut stmt = conn
                .prepare("SELECT phonetic, translation FROM ecdict WHERE sw = ?1 LIMIT 1")
                .ok()?;
            return stmt
                .query_row([&base_normalized], |row| {
                    Ok(LocalTranslation {
                        phonetic: row.get(0)?,
                        definitions: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    })
                })
                .ok();
        }

        None
    }

    /// 查询 CC-CEDICT（中→英）
    fn query_cedict(&self, normalized: &str) -> Option<LocalTranslation> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let conn = state.cedict.as_ref()?;

        // 精确匹配简体或繁体
        let mut stmt = conn
            .prepare(
                "SELECT pinyin, definitions FROM cedict WHERE simplified = ?1 OR traditional = ?1 LIMIT 1",
            )
            .ok()?;

        stmt.query_row([normalized], |row| {
            Ok(LocalTranslation {
                phonetic: row.get(0)?,
                definitions: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })
        .ok()
    }
}

/// 归一化：trim → lowercase → 移除所有空白
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<String>().to_lowercase()
}

/// 语言检测：含 CJK 字符 → 中→英；否则 → 英→中
pub fn detect_language(input: &str) -> TranslateDirection {
    let has_cjk = input.chars().any(|c| {
        let code = c as u32;
        (0x4E00..=0x9FFF).contains(&code)
            || (0x3400..=0x4DBF).contains(&code)
            || (0x20000..=0x2A6DF).contains(&code)
    });

    if has_cjk {
        TranslateDirection::ZhToEn
    } else {
        TranslateDirection::EnToZh
    }
}

/// 构建 Google 搜索 URL
pub fn build_google_url(query: &str) -> String {
    let encoded = urlencoding::encode(query.trim());
    format!("https://www.google.com/search?q={}", encoded)
}

/// 构建百度搜索 URL
pub fn build_baidu_url(query: &str) -> String {
    let encoded = urlencoding::encode(query.trim());
    format!("https://www.baidu.com/s?wd={}", encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 归一化_大小写与空白() {
        assert_eq!(normalize("  Hello  World  "), "helloworld");
        assert_eq!(normalize("Machine Learning"), "machinelearning");
    }

    #[test]
    fn 语言检测_英文() {
        assert_eq!(detect_language("hello"), TranslateDirection::EnToZh);
        assert_eq!(
            detect_language("machine learning"),
            TranslateDirection::EnToZh
        );
    }

    #[test]
    fn 语言检测_中文() {
        assert_eq!(detect_language("你好"), TranslateDirection::ZhToEn);
        assert_eq!(detect_language("单词"), TranslateDirection::ZhToEn);
    }

    #[test]
    fn 语言检测_混合() {
        // 含中文即中→英
        assert_eq!(detect_language("hello 你好"), TranslateDirection::ZhToEn);
    }

    #[test]
    fn google_url构建() {
        let url = build_google_url("hello world");
        assert!(url.contains("google.com/search"));
        assert!(url.contains("q=hello"));
    }

    #[test]
    fn baidu_url构建() {
        let url = build_baidu_url("你好");
        assert!(url.contains("baidu.com/s"));
        assert!(url.contains("wd="));
    }
}
