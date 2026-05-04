/**
 * E2E test: Multi-organization support in amux cloud (Playwright)
 *
 * Strategy: Block non-Clerk JS so the dashboard can't redirect away from cloud.amux.io,
 * complete the Clerk auth exchange, then unblock and run API tests via fetch.
 */
import { createRequire } from 'module';
const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
import { homedir } from 'os';
import { randomBytes } from 'crypto';

const CLOUD_URL = 'https://cloud.amux.io';
const PROFILE = `${homedir()}/.amux/playwright-auth/profile`;
const TEST_ORG = `e2e-test-${randomBytes(4).toString('hex')}`;

let passed = 0, failed = 0;
const results = [];

async function test(name, fn) {
  try {
    await fn();
    passed++;
    results.push({ name, ok: true });
    console.log(`  ✓ ${name}`);
  } catch (err) {
    failed++;
    results.push({ name, ok: false, err: err.message });
    console.log(`  ✗ ${name}: ${err.message}`);
  }
}

function assert(cond, msg) { if (!cond) throw new Error(msg); }

async function main() {
  console.log(`\n═══ amux Multi-Org E2E Test ═══\n`);
  console.log(`Test org: ${TEST_ORG}\n`);

  const ctx = await chromium.launchPersistentContext(PROFILE, {
    headless: true, ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 800 },
  });

  // Use a fresh page with JS blocked (except Clerk) so cloud.amux.io doesn't redirect
  const page = await ctx.newPage();
  await page.setExtraHTTPHeaders({ 'X-Amux-No-Redirect': '1' }); // in case gateway checks

  // Block inline/dashboard JS but allow Clerk CDN
  await page.route('**/*', async route => {
    const url = route.request().url();
    const type = route.request().resourceType();
    if (type === 'script' && !url.includes('clerk') && !url.includes('cdn.jsdelivr'))
      return route.abort();
    return route.continue();
  });

  await page.goto(CLOUD_URL, { waitUntil: 'domcontentloaded', timeout: 20000 });
  await page.waitForTimeout(6000); // let Clerk JS load

  // ── Auth exchange ──
  let authed = false;
  await test('Authenticate with cloud gateway', async () => {
    // First check if we already have a valid session cookie
    const identity = await page.evaluate(() =>
      fetch('/api/identity', { credentials: 'include' }).then(r => r.json()).catch(() => ({}))
    );
    if (identity.email && identity.is_cloud) {
      console.log(`    Already authenticated as ${identity.email}`);
      authed = true;
      return;
    }

    // Try Clerk token exchange
    const state = await page.evaluate(() => ({
      clerk: !!window.Clerk, user: !!window.Clerk?.user,
      email: window.Clerk?.user?.primaryEmailAddress?.emailAddress
    }));
    console.log(`    Clerk state: ${JSON.stringify(state)}`);

    if (!state.user) {
      throw new Error('No auth session. Log into cloud.amux.io in a browser, then re-run.');
    }

    const result = await page.evaluate(async () => {
      const token = await window.Clerk.session.getToken();
      const email = window.Clerk.user.primaryEmailAddress.emailAddress;
      const res = await fetch('/api/cloud-auth', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token, email })
      });
      return { status: res.status, ok: res.ok };
    });
    assert(result.ok, `Auth exchange failed: ${result.status}`);
    authed = true;
  });

  // Unblock all JS for API calls
  await page.unrouteAll();

  // Helper: call API from the cloud.amux.io origin
  const api = (method, path, body) => page.evaluate(async ({ method, path, body }) => {
    const opts = { method, credentials: 'include', headers: {} };
    if (body) { opts.headers['Content-Type'] = 'application/json'; opts.body = JSON.stringify(body); }
    const r = await fetch(path, opts);
    let json; try { json = await r.json(); } catch { json = null; }
    return { status: r.status, json };
  }, { method, path, body });

  if (!authed) {
    console.log('\n  ⚠ Not authenticated — skipping API tests');
    await ctx.close();
    console.log(`\n═══ Results: ${passed} passed, ${failed} failed ═══\n`);
    process.exit(1);
  }

  let orgId = null, inviteToken = null;

  // ── Test: Identity ──
  await test('Verify cloud identity', async () => {
    const r = await api('GET', '/api/identity');
    assert(r.status === 200, `status ${r.status}`);
    assert(r.json?.is_cloud === true, 'not cloud');
    assert(r.json?.email, 'no email');
    console.log(`    Email: ${r.json.email}`);
  });

  // ── Test: List orgs ──
  await test('List organizations', async () => {
    const r = await api('GET', '/api/gateway/orgs');
    assert(r.status === 200, `status ${r.status}`);
    assert(Array.isArray(r.json), 'not array');
    assert(r.json.length >= 1, 'no orgs');
    const personal = r.json.find(o => o.is_personal);
    assert(personal, 'no personal org');
    console.log(`    ${r.json.length} org(s), personal: ${personal.name}`);
  });

  // ── Test: Create org ──
  await test('Create new organization', async () => {
    const r = await api('POST', '/api/gateway/orgs', { name: TEST_ORG });
    assert(r.status === 201, `status ${r.status}: ${JSON.stringify(r.json)}`);
    assert(r.json?.id, 'no id');
    orgId = r.json.id;
    console.log(`    Created: ${orgId}`);
  });

  // ── Test: Org in list ──
  await test('New org appears in list', async () => {
    const r = await api('GET', '/api/gateway/orgs');
    assert(r.json.find(o => o.id === orgId), 'not found');
  });

  // ── Test: Org details ──
  await test('Get org details with members', async () => {
    const r = await api('GET', `/api/gateway/orgs/${orgId}`);
    assert(r.status === 200, `status ${r.status}`);
    assert(r.json?.name === TEST_ORG, `name mismatch: ${r.json?.name}`);
    assert(r.json?.members?.length === 1, `expected 1 member`);
    assert(r.json.members[0].role === 'owner', 'not owner');
    console.log(`    Members: ${r.json.members.length}, owner: ${r.json.members[0].email}`);
  });

  // ── Test: Switch org ──
  await test('Switch to new organization', async () => {
    const r = await api('POST', '/api/gateway/switch-org', { org_id: orgId });
    // This returns a redirect, but fetch follows it
    const orgs = await api('GET', '/api/gateway/orgs');
    const active = orgs.json?.find(o => o.active);
    assert(active?.id === orgId, `active org: ${active?.id}`);
    console.log(`    Active: ${active.name}`);
  });

  // ── Test: Create invite ──
  await test('Create invite', async () => {
    const r = await api('POST', '/api/org/invites', {
      org_id: orgId, email: 'test-e2e@example.com', role: 'member'
    });
    assert(r.status === 201, `status ${r.status}: ${JSON.stringify(r.json)}`);
    assert(r.json?.token, 'no token');
    inviteToken = r.json.token;
    console.log(`    Token: ${inviteToken.substring(0, 12)}...`);
  });

  // ── Test: List invites ──
  await test('List invites', async () => {
    const r = await api('GET', `/api/org/invites?org_id=${orgId}`);
    assert(r.status === 200, `status ${r.status}`);
    const found = r.json?.find(i => i.token === inviteToken);
    assert(found, 'invite not found');
    assert(found.email === 'test-e2e@example.com', `email: ${found.email}`);
    console.log(`    Found invite for ${found.email}`);
  });

  // ── Test: Self-invite page ──
  await test('Invite page shows self-invite warning', async () => {
    await page.goto(`${CLOUD_URL}/invite/${inviteToken}`, { waitUntil: 'domcontentloaded', timeout: 10000 });
    await page.waitForTimeout(1000);
    const text = await page.textContent('body');
    assert(text.includes('your own invite') || text.includes('Share it'), `unexpected: ${text.substring(0, 100)}`);
  });

  // ── Test: Update org ──
  await test('Update org name', async () => {
    // Navigate back to an API page to stay on cloud origin
    await page.goto(`${CLOUD_URL}/api/identity`, { waitUntil: 'domcontentloaded', timeout: 10000 });
    const r = await api('PATCH', `/api/gateway/orgs/${orgId}`, { name: TEST_ORG + '-updated' });
    assert(r.status === 200, `status ${r.status}`);
    const d = await api('GET', `/api/gateway/orgs/${orgId}`);
    assert(d.json?.name === TEST_ORG + '-updated', `name: ${d.json?.name}`);
    console.log(`    Renamed to: ${d.json.name}`);
  });

  // ── Test: Switch back ──
  await test('Switch back to personal', async () => {
    await api('POST', '/api/gateway/switch-org', { org_id: '' });
    const orgs = await api('GET', '/api/gateway/orgs');
    const active = orgs.json?.find(o => o.active);
    assert(active?.is_personal, `not personal: ${active?.name}`);
    console.log(`    Active: ${active.name}`);
  });

  // ── Test: Cannot access non-member org ──
  await test('Cannot access non-member org', async () => {
    const r = await api('GET', '/api/gateway/orgs/org_doesnt_exist_xyz');
    assert(r.status === 403 || r.status === 404, `status ${r.status}`);
  });

  // ── Test: Cannot delete personal ──
  await test('Cannot delete personal workspace', async () => {
    const orgs = await api('GET', '/api/gateway/orgs');
    const personal = orgs.json.find(o => o.is_personal);
    const r = await api('DELETE', `/api/gateway/orgs/${personal.id}`);
    assert(r.status === 400, `status ${r.status}`);
    console.log(`    Correctly blocked: ${r.json?.error}`);
  });

  // ── Cleanup ──
  console.log(`\n  Cleanup: deleting ${orgId}...`);
  if (orgId) {
    const del = await api('DELETE', `/api/gateway/orgs/${orgId}`);
    console.log(`    Delete: ${del.status} ${JSON.stringify(del.json)}`);
    const orgs = await api('GET', '/api/gateway/orgs');
    assert(!orgs.json.find(o => o.id === orgId), 'org still exists!');
    console.log(`    ✓ Cleaned up`);
  }

  await ctx.close();
  console.log(`\n═══ Results: ${passed} passed, ${failed} failed ═══\n`);
  process.exit(failed > 0 ? 1 : 0);
}

main().catch(err => {
  console.error('Fatal:', err.message);
  process.exit(1);
});
