import { useEffect, useRef, type ReactNode } from 'react';

export function Modal(props: {
  open: boolean;
  onClose: () => void;
  title: string;
  className: string;
  alert?: boolean;
  id?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const { open, onClose } = props;
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  }, [open]);
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return undefined;
    const outside = (e: MouseEvent) => {
      if (e.target === dialog && !props.alert) onClose();
    };
    dialog.addEventListener('click', outside);
    return () => dialog.removeEventListener('click', outside);
  }, [onClose, props.alert]);
  return (
    <dialog
      ref={ref}
      id={props.id}
      className={props.className}
      role={props.alert ? 'alertdialog' : undefined}
      aria-label={props.title}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      {open && props.children}
    </dialog>
  );
}
