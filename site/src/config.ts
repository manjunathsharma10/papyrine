/**
 * Single source of truth for everything that may change: the product name,
 * repo, licence and what is planned when. Nothing else in the site hardcodes these.
 * Feature milestones mirror docs/ROADMAP.md; Folio is pre-release, so every
 * feature is "Coming soon".
 */
export const product = {
  name: 'Folio',
  tagline: 'Everything you need from a PDF editor, without the bloat.',
  description:
    'Folio is a simple, lightweight, open-source PDF editor for Windows, macOS and Linux. Fast to launch, small to install, private and fully offline. Pre-release.',
  platforms: 'Windows, macOS and Linux',
};

export const repo = {
  owner: 'manjunathsharma10',
  name: 'folio',
  get url() {
    return `https://github.com/${this.owner}/${this.name}`;
  },
};

export const license = {
  spdx: 'MIT OR Apache-2.0',
  mitUrl: `${repo.url}/blob/main/LICENSE-MIT`,
  apacheUrl: `${repo.url}/blob/main/LICENSE-APACHE`,
};

/** Where the site is served. Override with SITE_URL / SITE_BASE for a custom domain. */
export const siteUrl = process.env.SITE_URL ?? `https://${repo.owner}.github.io`;
export const base = process.env.SITE_BASE ?? `/${repo.name}`;

/** Targets from docs/ARCHITECTURE.md §1.1. These are budgets, not measurements. */
export const targets = {
  installGoalMb: 30,
  installCeilingMb: 50,
  coldLaunchSeconds: 1.0,
};

/** Example numbers for the compression scene. Illustrative only. */
export const compressExample = { originalMb: 24, balancedMb: 3.1 };

export type Status = 'coming-soon';
export const status: Record<Status, string> = { 'coming-soon': 'Coming soon' };

export const milestones = [
  { version: 'v0.1', title: 'The everyday editor', items: 'Open, view, search, annotate, fill forms, organise pages, save safely' },
  { version: 'v0.2', title: 'Compression', items: 'Space audit, presets, before/after preview, batch compression' },
  { version: 'v0.3', title: 'Sign and protect', items: 'Electronic and digital signatures, encryption, redaction' },
];

/** Prefix a path with the deploy base ("/folio/…"). */
export const url = (path = '') => `${base.replace(/\/$/, '')}/${path.replace(/^\//, '')}`;
