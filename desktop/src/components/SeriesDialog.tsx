import { useEffect, useRef, type ReactNode } from "react";

export function SeriesDialog({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    const previousFocus = document.activeElement as HTMLElement | null;
    if (typeof dialog.showModal === "function") dialog.showModal();
    else dialog.setAttribute("open", "");
    return () => {
      if (typeof dialog.close === "function") dialog.close();
      previousFocus?.focus();
    };
  }, []);
  return (
    <dialog ref={ref} className="series-dialog" aria-label="新剧详情" onCancel={(event) => { event.preventDefault(); onClose(); }}>
      <header className="series-dialog-toolbar"><span>新剧详情</span><button type="button" className="secondary-button" onClick={onClose} autoFocus>关闭详情</button></header>
      {children}
    </dialog>
  );
}
