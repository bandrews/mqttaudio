import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ColorModeContext } from '../src/state/colorMode';
import { ColorModeToggle } from '../src/features/theme/ColorModeToggle';
import { ElectronConnection } from '../src/api/connection.electron';
import type { DaemonConnection } from '../src/api/connection';

describe('ColorModeToggle (W9)', () => {
  it('invokes toggle when clicked', async () => {
    const user = userEvent.setup();
    const toggle = vi.fn();
    render(
      <ColorModeContext.Provider value={{ mode: 'dark', toggle }}>
        <ColorModeToggle />
      </ColorModeContext.Provider>,
    );
    await user.click(screen.getByRole('button', { name: /toggle color mode/i }));
    expect(toggle).toHaveBeenCalledOnce();
  });
});

describe('Electron seam (DW2/DW13)', () => {
  it('the Electron connection stub conforms to DaemonConnection and throws until implemented', async () => {
    // Assigning to a DaemonConnection-typed binding proves the swap target conforms;
    // an Electron build replaces only this implementation, no UI change.
    const conn: DaemonConnection = new ElectronConnection({
      id: 'e',
      label: 'electron',
      baseUrl: 'electron://daemon',
    });
    expect(() => conn.get('/health')).toThrow(/not implemented/i);
    expect(() => conn.subscribe('/ws', { onMessage: () => {} })).toThrow(/not implemented/i);
  });
});
