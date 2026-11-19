# 检索与排名

## 启动器排序

```
score = match_score(kind) × frecency × source_weight
match_score: 完全相等 1.0 | 前缀 0.9 | 词首缩写 0.85 | 子串 0.7 | 模糊 0.4–0.6
frecency   = 1 + ln(1 + use_count) × 0.5^(days_since / 14)
source_weight: start_menu 1.0 | app_paths 0.95 | uwp 0.9 | builtin 1.05
```

同分按标题长度升序、再按 key 字典序，保证结果稳定。

## 应用索引

| 来源 | 方式 |
|---|---|
| 开始菜单 | 扫描 `.lnk`，解析目标与图标 |
| App Paths | 注册表 `HKLM/HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths` |
| UWP | 枚举 `shell:AppsFolder` |
| 内置命令 | 代码内置清单（锁屏/睡眠/设置/清空剪贴板/重建索引/退出） |

刷新策略：启动时后台构建；支持手动触发重建；默认不定时轮询。
