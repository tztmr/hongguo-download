import { COVER_MODEL_CHOICES, orderedCoverModels } from "./coverModels";
export function CoverModelPicker({ selected, legacy, onChange, disabled }: {
  selected?: string[]; legacy: string; onChange: (models: string[]) => void; disabled?: boolean;
}) {
  const order = orderedCoverModels(selected, legacy);
  const choices = [...new Set([...COVER_MODEL_CHOICES, ...order])];
  function toggle(model: string) {
    const next = order.includes(model) ? order.filter(m => m !== model) : [...order, model];
    if (next.length > 0 && next.length <= 4) onChange(next);
  }
  function move(index: number, direction: number) {
    const next = [...order]; [next[index], next[index + direction]] = [next[index + direction], next[index]]; onChange(next);
  }
  return <fieldset className="studio-model-picker" disabled={disabled}>
    <legend>Moyuu 封面模型 · 多选回退</legend>
    <div className="studio-model-choices">{choices.map(model => <label key={model}>
      <input type="checkbox" checked={order.includes(model)} disabled={order.includes(model) ? order.length === 1 : order.length >= 4} onChange={() => toggle(model)} />
      <span>{model}</span>
    </label>)}</div>
    <ol aria-label="封面模型回退顺序">{order.map((model, index) => <li key={model}>
      <span><b>{index === 0 ? "首选" : `备用 ${index}`}</b>{model}</span>
      <button type="button" aria-label={`上移 ${model}`} disabled={index === 0} onClick={() => move(index, -1)}>↑</button>
      <button type="button" aria-label={`下移 ${model}`} disabled={index === order.length - 1} onClick={() => move(index, 1)}>↓</button>
    </li>)}</ol>
    <small>按顺序尝试，每个模型最多一次；成功即停止，全部失败使用原封面。超时、连接中断、Key 无效、余额不足或限流时停止切换。切换模型可能产生额外费用。</small>
  </fieldset>;
}
