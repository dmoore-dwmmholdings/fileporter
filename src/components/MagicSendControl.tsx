import { FolderOpen, Sparkles } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

export type MagicChoice = 'files' | 'folder';

interface MagicSendControlProps {
  disabled?: boolean;
  onChoose: (choice: MagicChoice) => void;
}

export function MagicSendControl({ disabled = false, onChoose }: MagicSendControlProps) {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (isOpen) menuRef.current?.querySelector<HTMLButtonElement>('.magic-menu button')?.focus();
  }, [isOpen]);

  function choose(choice: MagicChoice) {
    setIsOpen(false);
    onChoose(choice);
  }

  function closeMenu() {
    setIsOpen(false);
    triggerRef.current?.focus();
  }

  return (
    <div className="magic-control" ref={menuRef}>
      <button
        ref={triggerRef}
        className="magic-button"
        type="button"
        disabled={disabled}
        aria-expanded={isOpen}
        aria-haspopup="menu"
        aria-controls="magic-send-options"
        onClick={() => setIsOpen((open) => !open)}
      >
        <Sparkles aria-hidden="true" size={22} />
        <span>Send something</span>
      </button>
      {isOpen && (
        <div id="magic-send-options" className="magic-menu" role="menu" aria-label="Browse files or folders" onKeyDown={(event) => {
          if (event.key === 'Escape') { event.preventDefault(); closeMenu(); }
        }}>
          <button type="button" role="menuitem" onClick={() => choose('files')}>
            <FolderOpen aria-hidden="true" size={18} /> Browse files
          </button>
          <button type="button" role="menuitem" onClick={() => choose('folder')}>
            <FolderOpen aria-hidden="true" size={18} /> Browse folder
          </button>
        </div>
      )}
    </div>
  );
}
