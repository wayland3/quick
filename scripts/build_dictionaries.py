#!/usr/bin/env python3
"""
构建翻译词典 SQLite 数据库

- ECDICT（英→中）：从 GitHub 下载 CSV，转换为 SQLite
- CC-CEDICT（中→英）：从 GitHub 下载原始文件，转换为 SQLite

输出：
- src-tauri/resources/dictionaries/ecdict.db
- src-tauri/resources/dictionaries/cedict.db
- src-tauri/resources/dictionaries/NOTICE.txt
"""

import csv
import json
import os
import re
import sqlite3
import sys
import urllib.request
from pathlib import Path

# 归一化规则：trim → lowercase → 移除所有空白
def normalize(s: str) -> str:
    return "".join(s.split()).lower()

def download_file(url: str, dest: Path):
    """下载文件，显示进度"""
    print(f"Downloading {url}...")
    with urllib.request.urlopen(url) as response:
        total = int(response.headers.get("Content-Length", 0))
        downloaded = 0
        chunk_size = 8192
        with open(dest, "wb") as f:
            while True:
                chunk = response.read(chunk_size)
                if not chunk:
                    break
                f.write(chunk)
                downloaded += len(chunk)
                if total > 0:
                    pct = downloaded * 100 / total
                    print(f"\r  {downloaded}/{total} ({pct:.1f}%)", end="", flush=True)
        print()

def build_ecdict(output_dir: Path):
    """构建 ECDICT（英→中）"""
    print("\n=== ECDICT (英→中) ===")
    
    # 下载 ECDICT CSV
    csv_url = "https://github.com/skywind3000/ECDICT/raw/master/ecdict.csv"
    csv_path = output_dir / "ecdict.csv"
    if not csv_path.exists():
        download_file(csv_url, csv_path)
    
    # 创建 SQLite
    db_path = output_dir / "ecdict.db"
    if db_path.exists():
        db_path.unlink()
    
    conn = sqlite3.connect(str(db_path))
    cur = conn.cursor()
    
    # 建表
    cur.execute("""
        CREATE TABLE ecdict (
            word TEXT NOT NULL,
            sw TEXT NOT NULL,
            phonetic TEXT,
            translation TEXT,
            pos TEXT
        )
    """)
    cur.execute("CREATE INDEX idx_sw ON ecdict(sw)")
    
    cur.execute("""
        CREATE TABLE ecdict_lemma (
            inflected TEXT NOT NULL,
            base TEXT NOT NULL
        )
    """)
    cur.execute("CREATE INDEX idx_inflected ON ecdict_lemma(inflected)")
    
    # 解析 CSV
    count = 0
    lemma_count = 0
    with open(csv_path, "r", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        for row in reader:
            word = row["word"].strip()
            sw = normalize(word)
            phonetic = row.get("phonetic", "").strip() or None
            translation = row.get("translation", "").strip() or None
            pos = row.get("pos", "").strip() or None
            exchange = row.get("exchange", "").strip()
            
            cur.execute(
                "INSERT INTO ecdict (word, sw, phonetic, translation, pos) VALUES (?, ?, ?, ?, ?)",
                (word, sw, phonetic, translation, pos)
            )
            count += 1
            
            # 从 exchange 字段构建 lemma 反向表
            if exchange:
                # exchange 格式如 "p:ran/t:ran/s:running/d:ran"
                for part in exchange.split("/"):
                    if ":" in part:
                        _, inflected = part.split(":", 1)
                        inflected = inflected.strip()
                        if inflected:
                            cur.execute(
                                "INSERT INTO ecdict_lemma (inflected, base) VALUES (?, ?)",
                                (normalize(inflected), word)
                            )
                            lemma_count += 1
    
    conn.commit()
    conn.close()
    
    db_size_mb = db_path.stat().st_size / 1024 / 1024
    print(f"  词条数: {count}")
    print(f"  Lemma 反向表: {lemma_count} 条")
    print(f"  数据库大小: {db_size_mb:.2f} MB")
    
    return count, db_size_mb

def build_cedict(output_dir: Path):
    """构建 CC-CEDICT（中→英）"""
    print("\n=== CC-CEDICT (中→英) ===")
    
    # 下载 CC-CEDICT 原始文件（使用 GitHub 镜像）
    raw_url = "https://github.com/ueda-keisuke/CC-CEDICT-MeCab/raw/master/cedict_ts.u8"
    raw_path = output_dir / "cedict_ts.u8"
    if not raw_path.exists():
        download_file(raw_url, raw_path)
    
    # 创建 SQLite
    db_path = output_dir / "cedict.db"
    if db_path.exists():
        db_path.unlink()
    
    conn = sqlite3.connect(str(db_path))
    cur = conn.cursor()
    
    cur.execute("""
        CREATE TABLE cedict (
            traditional TEXT NOT NULL,
            simplified TEXT NOT NULL,
            pinyin TEXT,
            definitions TEXT
        )
    """)
    cur.execute("CREATE INDEX idx_simplified ON cedict(simplified)")
    cur.execute("CREATE INDEX idx_traditional ON cedict(traditional)")
    
    # 解析原始文件
    # 格式：繁體 簡體 [pin1 yin1] /def1/def2/
    count = 0
    with open(raw_path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            
            # 解析：traditional simplified [pinyin] /definitions/
            match = re.match(r"^(\S+)\s+(\S+)\s+\[([^\]]+)\]\s+(.+)$", line)
            if not match:
                continue
            
            traditional = match.group(1)
            simplified = match.group(2)
            pinyin = match.group(3)
            defs_raw = match.group(4)
            
            # 提取定义（去掉首尾 /，按 / 分割）
            defs_raw = defs_raw.strip("/")
            definitions = "\n".join(d.strip() for d in defs_raw.split("/") if d.strip())
            
            cur.execute(
                "INSERT INTO cedict (traditional, simplified, pinyin, definitions) VALUES (?, ?, ?, ?)",
                (traditional, simplified, pinyin, definitions)
            )
            count += 1
    
    conn.commit()
    conn.close()
    
    db_size_mb = db_path.stat().st_size / 1024 / 1024
    print(f"  词条数: {count}")
    print(f"  数据库大小: {db_size_mb:.2f} MB")
    
    return count, db_size_mb

def write_notice(output_dir: Path):
    """生成 NOTICE.txt"""
    notice = """ECDICT
======
Source: https://github.com/skywind3000/ECDICT
License: MIT

MIT License

Copyright (c) 2017 Lin Wei

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


CC-CEDICT
=========
Source: https://www.mandarintools.com/cedict/
License: CC BY-SA 4.0

This work is licensed under the Creative Commons Attribution-ShareAlike 4.0 
International License. To view a copy of this license, visit 
http://creativecommons.org/licenses/by-sa/4.0/ or send a letter to Creative 
Commons, PO Box 1866, Mountain View, CA 94042, USA.
"""
    notice_path = output_dir / "NOTICE.txt"
    with open(notice_path, "w", encoding="utf-8") as f:
        f.write(notice)
    print(f"\nNOTICE.txt 已生成")

def main():
    output_dir = Path(__file__).parent.parent / "src-tauri" / "resources" / "dictionaries"
    output_dir.mkdir(parents=True, exist_ok=True)
    
    ecdict_count, ecdict_size = build_ecdict(output_dir)
    cedict_count, cedict_size = build_cedict(output_dir)
    write_notice(output_dir)
    
    print(f"\n=== 总计 ===")
    print(f"  ECDICT: {ecdict_count} 词条, {ecdict_size:.2f} MB")
    print(f"  CC-CEDICT: {cedict_count} 词条, {cedict_size:.2f} MB")
    print(f"  合计: {(ecdict_size + cedict_size):.2f} MB")

if __name__ == "__main__":
    main()
