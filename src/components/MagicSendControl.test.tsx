import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MagicSendControl } from './MagicSendControl';

it('opens the compact picker and forwards the selected kind', () => {
  const onChoose = vi.fn();
  render(<MagicSendControl onChoose={onChoose} />);
  fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
  expect(screen.getByRole('menu', { name: 'Browse files or folders' })).toBeVisible();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse folder' }));
  expect(onChoose).toHaveBeenCalledWith('folder');
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});

it('is keyboard accessible and restores focus after Escape', async () => {
  render(<MagicSendControl onChoose={vi.fn()} />);
  const trigger = screen.getByRole('button', { name: 'Send something' });
  fireEvent.click(trigger);
  const menu = screen.getByRole('menu');
  await waitFor(() => expect(screen.getByRole('menuitem', { name: 'Browse files' })).toHaveFocus());
  fireEvent.keyDown(menu, { key: 'Escape' });
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});
