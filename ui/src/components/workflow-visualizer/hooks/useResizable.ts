import { useState, useCallback, useEffect, RefObject } from 'react';

interface UseResizableOptions {
  /** Ref to the container element used for position calculations */
  containerRef: RefObject<HTMLElement | null>;
  /** Direction of resize: 'horizontal' measures clientX, 'vertical' measures clientY as percentage */
  direction: 'horizontal' | 'vertical';
  /** Minimum size (px for horizontal, % for vertical) */
  min: number;
  /** Maximum size (px for horizontal, % for vertical) */
  max: number;
  /** Initial size (px for horizontal, % for vertical) */
  initial: number;
}

interface UseResizableReturn {
  /** Current size value (px for horizontal, % for vertical) */
  size: number;
  /** Whether the user is currently dragging */
  isDragging: boolean;
  /** Mouse down handler to attach to the divider element */
  onMouseDown: (e: React.MouseEvent) => void;
}

/**
 * Hook that encapsulates drag-to-resize logic for panels.
 * Handles document-level mousemove/mouseup listeners and cleanup.
 */
export function useResizable({
  containerRef,
  direction,
  min,
  max,
  initial,
}: UseResizableOptions): UseResizableReturn {
  const [size, setSize] = useState(initial);
  const [isDragging, setIsDragging] = useState(false);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  useEffect(() => {
    if (!isDragging) return;

    const handleMouseMove = (e: MouseEvent) => {
      const el = containerRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();

      if (direction === 'horizontal') {
        const newSize = e.clientX - rect.left;
        setSize(Math.max(min, Math.min(max, newSize)));
      } else {
        const relativeY = e.clientY - rect.top;
        const percentage = (relativeY / rect.height) * 100;
        setSize(Math.max(min, Math.min(max, percentage)));
      }
    };

    const handleMouseUp = () => setIsDragging(false);

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isDragging, containerRef, direction, min, max]);

  return { size, isDragging, onMouseDown };
}
