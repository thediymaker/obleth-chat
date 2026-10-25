// Public site branding. Edit this file and reload the page; no build is needed.
// This file is served to every visitor. Never put passwords or secrets here.
// Text is plain text, not HTML. Use \n for a line break; '' hides optional copy.
window.OblethChatConfig = {
  // \n stacks a long name beside the logo, e.g. 'Research\nComputing'; the
  // browser title, footer and sign-in text show it on one line.
  siteName: 'Obleth Chat',
  siteSubtitle: '',
  assistantName: 'Assistant',
  description: 'A space to ask questions, explore ideas, and make something new.',

  // Logo beside the site name. Swap in local image paths, e.g. './assets/custom/logo.svg':
  // 'light' shows on the light theme, 'dark' on the dark theme, and one image covers both.
  // Leave both blank for a text-only brand.
  logo: { light: './assets/obleth-ink.svg', dark: './assets/obleth.svg', alt: '' },

  // Browser tab icon. Blank uses a neutral chat icon; one image covers both themes.
  favicon: { light: './assets/obleth-ink.svg', dark: './assets/obleth.svg' },

  // Built-in names from icons.js, e.g. 'chat', 'book', 'sun', 'bolt', or 'cube'.
  // Use '' to hide an icon. These icons are separate from the optional site logo.
  icons: { greeting: 'chat', assistant: 'chat', loading: 'chat' },

  login: {
    eyebrow: 'A space for every idea',
    title: 'A little space\nfor big ideas.',
    description: 'Ask questions, explore ideas, and make something new.\nYour conversations, all in one place.',
    footer: '',
    welcomeTitle: 'Make yourself at home.',
    welcomeDescription: 'Sign in to your personal workspace.',
    help: 'Need an account? Your administrator can create one for you.',
  },

  // Links in the sign-in page footer, e.g. help, privacy or terms. Use [] for none.
  links: [
    { label: 'Documentation', url: 'https://www.obleth.com/docs/chat/introduction' },
    { label: 'Obleth', url: 'https://www.obleth.com' },
  ],

  home: {
    eyebrow: 'Your space to explore',
    title: 'What’s on your mind?',
    description: 'Ask a question, explore an idea, or start something new.',
    footer: '',
  },

  // 'system', 'light', or 'dark'. A person's saved preference takes priority.
  appearance: { defaultTheme: 'system' },
};
