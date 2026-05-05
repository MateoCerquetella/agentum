// SPA mode: no SSR / no prerender. The Rust backend embeds index.html and
// serves it as a fallback for every non-/api/* path; client-side routing
// takes over from there.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';
