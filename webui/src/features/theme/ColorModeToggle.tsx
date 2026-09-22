// Light/dark color-mode toggle (Sprint W9).

import IconButton from '@mui/material/IconButton';
import Brightness4Icon from '@mui/icons-material/Brightness4';
import Brightness7Icon from '@mui/icons-material/Brightness7';
import { useColorMode } from '../../state/colorMode';

export function ColorModeToggle() {
  const { mode, toggle } = useColorMode();
  return (
    <IconButton size="small" color="inherit" onClick={toggle} aria-label="toggle color mode">
      {mode === 'dark' ? <Brightness7Icon fontSize="small" /> : <Brightness4Icon fontSize="small" />}
    </IconButton>
  );
}
