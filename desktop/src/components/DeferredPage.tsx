import { Component, Suspense, lazy, useState, type ComponentType, type ReactNode } from "react";

type BoundaryProps = { title: string; hidden: boolean; onRetry(): void; children: ReactNode };

class PageBoundary extends Component<BoundaryProps, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    if (!this.state.failed) return this.props.children;
    if (this.props.hidden) return null;
    return <main className="deferred-page">
      <h1>{this.props.title}</h1>
      <p role="alert">页面加载失败，请重试。后台任务会继续运行。</p>
      <button type="button" className="secondary-button" onClick={this.props.onRetry}>重新加载页面</button>
    </main>;
  }
}

export function deferPage<Props extends { hidden?: boolean }>(title: string, load: () => Promise<{ default: ComponentType<Props> }>) {
  const initialPage = lazy(load);
  return function DeferredPage(props: Props) {
    const [{ Page, attempt }, setPage] = useState({ Page: initialPage, attempt: 0 });
    return <PageBoundary key={attempt} title={title} hidden={!!props.hidden}
      onRetry={() => setPage({ Page: lazy(load), attempt: attempt + 1 })}>
      <Suspense fallback={props.hidden ? null : <main className="deferred-page"><h1>{title}</h1><p role="status">正在加载{title}…</p></main>}>
        <Page {...props} />
      </Suspense>
    </PageBoundary>;
  };
}
