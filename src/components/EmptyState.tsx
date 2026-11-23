interface EmptyStateProps {
  /** 当前搜索词；为空表示空历史 */
  query: string;
  /** 当前筛选标签，用于空态文案 */
  filterLabel: string;
}

/** 空态 / 无搜索结果：侘寂风，简洁克制 */
export function EmptyState({ query, filterLabel }: EmptyStateProps) {
  if (query.trim()) {
    return (
      <div className="qw-empty" role="status">
        <span className="qw-empty-icon" aria-hidden="true">
          ◯
        </span>
        <p className="qw-empty-title">无果</p>
        <p className="qw-empty-hint">
          未找到「{query.trim()}」· 按 Tab 切换筛选
        </p>
      </div>
    );
  }

  return (
    <div className="qw-empty" role="status">
      <span className="qw-empty-icon" aria-hidden="true">
        ◯
      </span>
      <p className="qw-empty-title">
        {filterLabel === '全部' ? '空' : `「${filterLabel}」为空`}
      </p>
      <p className="qw-empty-hint">复制内容后会出现在这里</p>
    </div>
  );
}
