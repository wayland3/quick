# 数据库

## PRAGMA

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
PRAGMA user_version;  -- 迁移版本
```

## 核心表

### clipboard_item（剪贴板条目）

```sql
CREATE TABLE clipboard_item (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  kind           TEXT    NOT NULL CHECK (kind IN ('text','html','rtf','image','files')),
  hash           TEXT    NOT NULL,
  preview        TEXT    NOT NULL,
  text_body      TEXT,
  html           TEXT,
  rtf            TEXT,
  blob_path      TEXT,
  thumb_path     TEXT,
  file_list      TEXT,              -- JSON: [{path,exists}]
  byte_size      INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL,  -- unix ms
  last_copied_at INTEGER NOT NULL,  -- unix ms，TTL 依据
  copy_count     INTEGER NOT NULL DEFAULT 1,
  pinned         INTEGER NOT NULL DEFAULT 0,
  truncated      INTEGER NOT NULL DEFAULT 0
);
```

### clipboard_fts（全文搜索）

独立 FTS5，rowid = clipboard_item.id：

```sql
CREATE VIRTUAL TABLE clipboard_fts USING fts5(
  preview, text_body, tokenize = 'trigram'
);
```

### 其余表

`settings`、`app_index`、`usage_stat`、`meta` 随对应功能以独立 migration 落地。

## 迁移

- `PRAGMA user_version` 为当前版本
- 每条迁移在独立事务内；失败回滚并保留原版本
- 每条迁移必须配测试
- 禁止修改已发布的迁移函数
