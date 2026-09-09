import { useState } from "react";
import type { CategoryGroup } from "../types";
import "./category.css";

type CategoryFilterProps = { groups: CategoryGroup[] } & (
  | { selectedId: string; onSelect(id: string): void; values?: never; onChange?: never }
  | { values: Record<string, string>; onChange(group: string, id: string): void; selectedId?: never; onSelect?: never }
);

export function CategoryFilter(props: CategoryFilterProps) {
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  if (!props.groups.length) return null;
  return (
    <div className="category-filter category-facets" aria-label="分类筛选">
      {props.groups.map((group) => {
        const selected = props.values ? props.values[group.id] ?? group.items[0]?.id : props.selectedId;
        const options = expanded[group.id] ? group.items : group.items.slice(0, 9);
        const selectedItem = group.items.find((item) => item.id === selected);
        const visible = selectedItem && !options.includes(selectedItem) ? [...options.slice(0, 8), selectedItem] : options;
        return (
          <div className="category-facet" key={group.id}>
            <span className="category-facet-label">{group.name}</span>
            <div className="category-item-row" role="group" aria-label={group.name}>
              {visible.map((item) => <button type="button" key={item.id} aria-pressed={item.id === selected} className={item.id === selected ? "active" : ""} onClick={() => props.onChange ? props.onChange(group.id, item.id) : props.onSelect(item.id)}>{item.name}</button>)}
              {group.items.length > 9 ? <button type="button" className="category-expand" aria-expanded={Boolean(expanded[group.id])} onClick={() => setExpanded((state) => ({ ...state, [group.id]: !state[group.id] }))}>{expanded[group.id] ? "收起" : `更多 ${group.items.length - 9}`}</button> : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}
