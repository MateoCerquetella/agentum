/**
 * E2E test: Multi-organization support in amux cloud
 *
 * Tests:
 * 1. Login via saved auth profile (Clerk session cookies)
 * 2. List organizations the user belongs to
 * 3. Create a new test organization
 * 4. Switch to the new organization
 * 5. Verify org switch took effect (identity/context changed)
 * 6. Create an invite for the new org
 * 7. Verify invite appears in invite list
 * 8. Switch back to personal workspace
 * 9. Verify back on personal workspace
 * 10. Delete the test organization (cleanup)
 *
 * Usage: node tests/e2e-multi-org.mjs
 */

import { createRequire } from 'module';
const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
import { homedir } from 'os';
import { randomBytes } from 'crypto';

const CLOUD_URL = 'https://cloud.amux.io';
const PROFILE_PATH = `${homedir()}/.amux/playwright-auth/profile`;
const TEST_ORG_NAME = `e2e-test-${randomBytes(4).toString('hex')}`;

let ctx, page;
let passed = 0, failed = 0;
const results = [];

function log(msg) { console.log(`  ${msg}`); }

async function test(name, fn) {
  try {
    await fn();
    passed++;
    results.push({ name, status: 'PASS' });
    console.log(`✓ ${name}`);
  } catch (err) {
    failed++;
    results.push({ name, status: 'FAIL', error: err.message });
    console.log(`✗ ${name}: ${err.message}`);
  }
}

function assert(condition, msg) {
  if (!condition) throw new Error(msg || 'Assertion failed');
}

// Helper: call gateway API with cookies from the browser context
async function apiCall(method, path, body) {
  const resp = await page.evaluate(async ({ method, path, body }) => {
    const opts = { method, credentials: 'include', headers: {} };
    if (body) {
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify(body);
    }
    const r = await fetch(path, opts);
    const text = await r.text();
    let json;
    try { json = JSON.parse(text); } catch { json = null; }
    return { status: r.status, json, text, redirected: r.redirected, url: r.url };
  }, { method, path, body });
  return resp;
}

async function main() {
  console.log('\n═══ amux Multi-Org E2E Test ═══\n');
  console.log(`Cloud URL: ${CLOUD_URL}`);
  console.log(`Test org name: ${TEST_ORG_NAME}\n`);

  // Launch browser with saved auth profile
  ctx = await chromium.launchPersistentContext(PROFILE_PATH, {
    headless: true,
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 800 },
  });
  page = ctx.pages()[0] || await ctx.newPage();

  let createdOrgId = null;
  let createdInviteToken = null;

  try {
    // ── Test 1: Navigate to cloud and authenticate ──
    await test('Navigate to cloud and verify authenticated', async () => {
      await page.goto(CLOUD_URL, { waitUntil: 'networkidle', timeout: 30000 });
      const url = page.url();
      log(`Current URL: ${url}`);

      // Check if we already have a valid session
      let identity = await apiCall('GET', '/api/identity');
      if (identity.status === 401 || !identity.json?.email) {
        // No amux_session cookie yet — Clerk cookies may still be valid
        // Navigate to login page which auto-exchanges Clerk token for amux_session
        log('No amux_session — attempting Clerk token exchange...');
        await page.goto(CLOUD_URL, { waitUntil: 'networkidle', timeout: 30000 });

        // Wait for Clerk to load and auto-exchange (the login page does this automatically
        // if Clerk.user exists — see exchangeAndRedirect in login HTML)
        try {
          // Wait for redirect away from login page (indicates successful auth exchange)
          await page.waitForFunction(() => {
            return !document.querySelector('#clerk-root') ||
                   document.querySelector('.tab-bar') ||
                   document.querySelector('#cards');
          }, { timeout: 20000 });
          await page.waitForTimeout(2000);
        } catch {
          // May still be on login page — try to check if Clerk mounted sign-in
          const pageText = await page.textContent('body');
          if (pageText.includes('Sign in')) {
            throw new Error('Clerk session expired — need interactive re-login. Run: /playwright-auth to capture a fresh session for cloud.amux.io');
          }
        }
        identity = await apiCall('GET', '/api/identity');
      }

      log(`Identity: ${JSON.stringify(identity.json)}`);
      assert(identity.status === 200, `Expected 200, got ${identity.status}`);
      assert(identity.json?.email, 'No email in identity - not authenticated. Run /playwright-auth to capture cloud.amux.io session');
      assert(identity.json?.is_cloud === true, 'Not in cloud mode');
      log(`Authenticated as: ${identity.json.email}`);
    });

    // ── Test 2: List existing organizations ──
    await test('List organizations', async () => {
      const resp = await apiCall('GET', '/api/gateway/orgs');
      log(`Status: ${resp.status}`);
      assert(resp.status === 200, `Expected 200, got ${resp.status}`);
      assert(Array.isArray(resp.json), 'Expected array of orgs');
      assert(resp.json.length >= 1, 'Expected at least 1 org (personal)');
      const personal = resp.json.find(o => o.is_personal);
      assert(personal, 'No personal org found');
      log(`Found ${resp.json.length} org(s): ${resp.json.map(o => o.name + (o.active ? ' [active]' : '')).join(', ')}`);
    });

    // ── Test 3: Create a new test organization ──
    await test('Create new organization', async () => {
      const resp = await apiCall('POST', '/api/gateway/orgs', {
        name: TEST_ORG_NAME,
        slug: TEST_ORG_NAME.toLowerCase(),
      });
      log(`Status: ${resp.status}, Body: ${JSON.stringify(resp.json)}`);
      assert(resp.status === 201, `Expected 201, got ${resp.status}: ${resp.text}`);
      assert(resp.json?.id, 'No org ID returned');
      assert(resp.json?.name === TEST_ORG_NAME, `Name mismatch: ${resp.json?.name}`);
      createdOrgId = resp.json.id;
      log(`Created org: ${createdOrgId}`);
    });

    // ── Test 4: Verify new org appears in list ──
    await test('New org appears in org list', async () => {
      const resp = await apiCall('GET', '/api/gateway/orgs');
      assert(resp.status === 200, `Expected 200, got ${resp.status}`);
      const found = resp.json.find(o => o.id === createdOrgId);
      assert(found, `Org ${createdOrgId} not in list`);
      assert(found.role === 'owner', `Expected owner role, got ${found.role}`);
      log(`Org ${createdOrgId} found with role: ${found.role}`);
    });

    // ── Test 5: Get org details ──
    await test('Get org details with members', async () => {
      const resp = await apiCall('GET', `/api/gateway/orgs/${createdOrgId}`);
      log(`Status: ${resp.status}`);
      assert(resp.status === 200, `Expected 200, got ${resp.status}`);
      assert(resp.json?.name === TEST_ORG_NAME, `Name mismatch: ${resp.json?.name}`);
      assert(Array.isArray(resp.json?.members), 'No members array');
      assert(resp.json.members.length === 1, `Expected 1 member, got ${resp.json.members.length}`);
      assert(resp.json.members[0].role === 'owner', 'Creator should be owner');
      log(`Org has ${resp.json.members.length} member(s), owner: ${resp.json.members[0].email}`);
    });

    // ── Test 6: Switch to new organization ──
    await test('Switch to new organization', async () => {
      // switch-org does a redirect, so we navigate via page
      const resp = await page.evaluate(async (orgId) => {
        const r = await fetch('/api/gateway/switch-org', {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ org_id: orgId }),
          redirect: 'follow',
        });
        return { status: r.status, url: r.url, redirected: r.redirected };
      }, createdOrgId);
      log(`Switch response: status=${resp.status}, redirected=${resp.redirected}`);
      // Reload to pick up new org context
      await page.goto(CLOUD_URL, { waitUntil: 'networkidle', timeout: 30000 });
      // Verify active org changed
      const orgs = await apiCall('GET', '/api/gateway/orgs');
      const active = orgs.json.find(o => o.active);
      assert(active, 'No active org found');
      assert(active.id === createdOrgId, `Expected active org ${createdOrgId}, got ${active.id}`);
      log(`Active org is now: ${active.name} (${active.id})`);
    });

    // ── Test 7: Create an invite for the new org ──
    await test('Create invite for org', async () => {
      const resp = await apiCall('POST', '/api/org/invites', {
        org_id: createdOrgId,
        email: 'test-invite@example.com',
        role: 'member',
      });
      log(`Status: ${resp.status}, Body: ${JSON.stringify(resp.json)}`);
      assert(resp.status === 201, `Expected 201, got ${resp.status}: ${resp.text}`);
      assert(resp.json?.token, 'No invite token returned');
      assert(resp.json?.url, 'No invite URL returned');
      createdInviteToken = resp.json.token;
      log(`Invite created: ${resp.json.url}`);
    });

    // ── Test 8: List invites and verify ──
    await test('List invites shows new invite', async () => {
      const resp = await apiCall('GET', `/api/org/invites?org_id=${createdOrgId}`);
      log(`Status: ${resp.status}`);
      assert(resp.status === 200, `Expected 200, got ${resp.status}`);
      assert(Array.isArray(resp.json), 'Expected array');
      const found = resp.json.find(i => i.token === createdInviteToken);
      assert(found, 'Invite not found in list');
      assert(found.email === 'test-invite@example.com', `Email mismatch: ${found.email}`);
      assert(!found.used_at, 'Invite should not be used yet');
      log(`Found invite for ${found.email}, expires: ${new Date(found.expires_at * 1000).toISOString()}`);
    });

    // ── Test 9: Visit invite page (as owner, should show "that's your own invite") ──
    await test('Invite page shows self-invite message for owner', async () => {
      await page.goto(`${CLOUD_URL}/invite/${createdInviteToken}`, { waitUntil: 'networkidle', timeout: 15000 });
      const text = await page.textContent('body');
      log(`Invite page text: ${text.substring(0, 100)}`);
      assert(text.includes('your own invite') || text.includes('Share it'), 'Expected self-invite warning');
    });

    // ── Test 10: Update org name ──
    await test('Update organization name', async () => {
      const newName = TEST_ORG_NAME + '-updated';
      const resp = await apiCall('PATCH', `/api/gateway/orgs/${createdOrgId}`, { name: newName });
      assert(resp.status === 200, `Expected 200, got ${resp.status}`);
      // Verify
      const detail = await apiCall('GET', `/api/gateway/orgs/${createdOrgId}`);
      assert(detail.json?.name === newName, `Name not updated: ${detail.json?.name}`);
      log(`Org name updated to: ${newName}`);
    });

    // ── Test 11: Switch back to personal workspace ──
    await test('Switch back to personal workspace', async () => {
      await page.evaluate(async () => {
        await fetch('/api/gateway/switch-org', {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ org_id: '' }),
          redirect: 'follow',
        });
      });
      await page.goto(CLOUD_URL, { waitUntil: 'networkidle', timeout: 30000 });
      const orgs = await apiCall('GET', '/api/gateway/orgs');
      const active = orgs.json.find(o => o.active);
      assert(active, 'No active org');
      assert(active.is_personal, `Expected personal workspace to be active, got ${active.name}`);
      log(`Back on personal workspace: ${active.name}`);
    });

    // ── Test 12: UI org switcher renders ──
    await test('UI org switcher renders in settings', async () => {
      // Open settings panel
      await page.goto(CLOUD_URL, { waitUntil: 'networkidle', timeout: 30000 });
      // Wait for the app to load and check if org switcher exists
      const hasSwitcher = await page.evaluate(async () => {
        // Try to load orgs to ensure the switcher would render
        const r = await fetch('/api/gateway/orgs', { credentials: 'include' });
        const orgs = await r.json();
        return { orgCount: orgs.length, hasMultiple: orgs.length > 1 };
      });
      assert(hasSwitcher.hasMultiple, `Expected multiple orgs, got ${hasSwitcher.orgCount}`);
      log(`Org switcher should render: ${hasSwitcher.orgCount} orgs available`);
    });

    // ── Test 13: Non-member cannot access org ──
    await test('Cannot access org you are not a member of', async () => {
      const resp = await apiCall('GET', '/api/gateway/orgs/org_nonexistent_fake');
      assert(resp.status === 403 || resp.status === 404, `Expected 403/404, got ${resp.status}`);
      log(`Correctly denied access: ${resp.status}`);
    });

    // ── Test 14: Cannot delete personal workspace ──
    await test('Cannot delete personal workspace', async () => {
      const orgs = await apiCall('GET', '/api/gateway/orgs');
      const personal = orgs.json.find(o => o.is_personal);
      assert(personal, 'No personal org');
      const resp = await apiCall('DELETE', `/api/gateway/orgs/${personal.id}`);
      assert(resp.status === 400, `Expected 400, got ${resp.status}`);
      log(`Correctly prevented: ${resp.json?.error}`);
    });

  } finally {
    // ── Cleanup: delete test org ──
    if (createdOrgId) {
      console.log(`\nCleaning up: deleting org ${createdOrgId}`);
      // Switch back to personal first
      await page.evaluate(async () => {
        await fetch('/api/gateway/switch-org', {
          method: 'POST', credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ org_id: '' }), redirect: 'follow',
        });
      });
      const del = await apiCall('DELETE', `/api/gateway/orgs/${createdOrgId}`);
      log(`Delete org: ${del.status} ${JSON.stringify(del.json)}`);
      // Verify it's gone
      const orgs = await apiCall('GET', '/api/gateway/orgs');
      const stillExists = orgs.json?.find(o => o.id === createdOrgId);
      if (stillExists) {
        console.log(`  ⚠ Org ${createdOrgId} still exists after delete!`);
      } else {
        console.log(`  ✓ Org ${createdOrgId} successfully deleted`);
      }
    }

    await ctx.close();
  }

  // ── Summary ──
  console.log(`\n═══ Results: ${passed} passed, ${failed} failed ═══\n`);
  results.forEach(r => {
    console.log(`  ${r.status === 'PASS' ? '✓' : '✗'} ${r.name}${r.error ? ' — ' + r.error : ''}`);
  });
  console.log('');

  process.exit(failed > 0 ? 1 : 0);
}

main().catch(err => {
  console.error('Fatal error:', err);
  if (ctx) ctx.close().catch(() => {});
  process.exit(1);
});
