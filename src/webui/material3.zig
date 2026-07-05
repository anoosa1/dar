//! Material Design 3 theme via Tailwind CDN config.
//! Defines color tokens, border radius, shadows as Tailwind theme extensions.
const std = @import("std");

/// Returns the Material 3 Tailwind config + font import for the <head>.
pub fn head() []const u8 {
    return
        \\<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:opsz,wght,FILL,GRAD@24,400,0,0" />
        \\<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Roboto:wght@400;500;700&display=swap" />
        \\<script src="https://cdn.tailwindcss.com"></script>
        \\<script>
        \\tailwind.config = {
        \\  theme: {
        \\    extend: {
        \\      colors: {
        \\        primary: '#6750A4',
        \\        'on-primary': '#FFFFFF',
        \\        'primary-container': '#EADDFF',
        \\        'on-primary-container': '#21005D',
        \\        surface: '#FEF7FF',
        \\        'surface-dim': '#DED8E1',
        \\        'surface-container': '#F3EDF7',
        \\        'surface-container-high': '#ECE6F0',
        \\        'surface-container-highest': '#E6E0E9',
        \\        'on-surface': '#1D1B20',
        \\        'on-surface-variant': '#49454F',
        \\        outline: '#79747E',
        \\        'outline-variant': '#CAC4D0',
        \\        error: '#B3261E',
        \\        'error-container': '#F9DEDC',
        \\        'on-error': '#FFFFFF',
        \\        'on-error-container': '#410E0B',
        \\        background: '#FEF7FF',
        \\        'on-background': '#1D1B20',
        \\      },
        \\      fontFamily: { sans: ['Roboto', 'system-ui', 'sans-serif'] },
        \\      borderRadius: { sm: '8px', md: '12px', lg: '16px', xl: '20px', '2xl': '24px', full: '28px' },
        \\      boxShadow: {
        \\        '1': '0 1px 2px 0 rgba(0,0,0,.08), 0 1px 3px 1px rgba(0,0,0,.08)',
        \\        '2': '0 1px 2px 0 rgba(0,0,0,.12), 0 2px 6px 2px rgba(0,0,0,.08)',
        \\        '3': '0 1px 3px 0 rgba(0,0,0,.16), 0 4px 8px 3px rgba(0,0,0,.08)',
        \\      },
        \\    }
        \\  }
        \\};
        \\</script>
    ;
}
