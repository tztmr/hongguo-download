import { useEffect, useMemo, useState } from "react";
import type { CategoryGroup } from "../types";

type CategoryFilterProps = {
  groups: CategoryGroup[];
  selectedId: string;
  onSelect(id: string): void;
};

function initialGroup(groups: CategoryGroup[], selectedId: string) {
  return groups.find((group) => group.items.some((item) => item.id === selectedId))?.id
    ?? groups[0]?.id
    ?? "";
}

export function CategoryFilter({ groups, selectedId, onSelect }: CategoryFilterProps) {
  const [activeGroupId, setActiveGroupId] = useState(() => initialGroup(groups, selectedId));
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    if (!groups.some((group) => group.id === activeGroupId)) {
      setActiveGroupId(initialGroup(groups, selectedId));
      setExpanded(false);
    }
  }, [activeGroupId, groups, selectedId]);

  const activeGroup = groups.find((group) => group.id === activeGroupId) ?? groups[0];
  const visibleItems = useMemo(() => {
    if (!activeGroup || expanded || activeGroup.items.length <= 10) {
      return activeGroup?.items ?? [];
    }
    const selected = activeGroup.items.find((item) => item.id === selectedId);
    if (selected && !activeGroup.items.slice(0, 10).includes(selected)) {
      return [...activeGroup.items.slice(0, 9), selected];
    }
    return activeGroup.items.slice(0, 10);
  }, [activeGroup, expanded, selectedId]);

  if (!groups.length || !activeGroup) return null;

  return (
    <div className="category-filter" aria-label="首页分类">
      <div className="category-group-row" role="group" aria-label="分类分组">
        {groups.map((group) => (
          <button
            type="button"
            className={group.id === activeGroup.id ? "active" : ""}
            aria-pressed={group.id === activeGroup.id}
            key={group.id}
            onClick={() => {
              setActiveGroupId(group.id);
              setExpanded(false);
            }}
          >
            {group.name}
          </button>
        ))}
      </div>
      <div className="category-item-row" role="group" aria-label={activeGroup.name}>
        {visibleItems.map((item) => (
          <button
            type="button"
            className={item.id === selectedId ? "active" : ""}
            aria-pressed={item.id === selectedId}
            key={item.id}
            onClick={() => onSelect(item.id)}
          >
            {item.name}
          </button>
        ))}
        {activeGroup.items.length > 10 ? (
          <button type="button" className="category-expand" onClick={() => setExpanded((value) => !value)}>
            {expanded ? "收起分类" : `展开全部 ${activeGroup.items.length} 项`}
          </button>
        ) : null}
      </div>
    </div>
  );
}
