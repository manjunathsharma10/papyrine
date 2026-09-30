import { defineConfig } from 'astro/config';
import { siteUrl, base } from './src/config.ts';

export default defineConfig({
  site: siteUrl,
  base,
  output: 'static',
  trailingSlash: 'always',
  // Inline all CSS: one fewer render-blocking request, and the whole sheet is a few KB gzipped.
  build: { inlineStylesheets: 'always' },
  devToolbar: { enabled: false },
});
