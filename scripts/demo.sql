-- Demo data for scripts/demo.sh: invented people, example.com / .example domains only.
-- Times are relative to now, so the demo always looks current. Run on a fresh, empty hub database.

-- Sign-in: user "demo", password "demo".
INSERT INTO users VALUES ('demo', '$2b$10$AgentumDemoSeedSaltSaePjXnZlP9alNzomPOQObo.31FfMXyT3W');
INSERT INTO kv VALUES ('settings', json('{"env": {"CERQ_MY_NAME": "Alex Rivera"}, "ui": {"look": {"theme": "studio", "mode": "system"}}}'));

-- Mail: three accounts on an IMAP server that isn't there (port 9 refuses), so the sync never
-- reaches anything or replaces the rows below. No password yet: demo.sh sets one (which makes
-- them usable) after the hub's first, empty sync, so Mail doesn't warn that they aren't syncing.
INSERT INTO email_accounts (id, name, is_default, enabled, imap_host, imap_port, imap_user, imap_password, smtp_host, smtp_port, smtp_user, smtp_password, from_address, display_name) VALUES
  ('personal', 'Personal',  1, 1, '127.0.0.1', 9, 'alex.rivera@example.com', '', '127.0.0.1', 9, 'alex.rivera@example.com', 'demo', 'alex.rivera@example.com', 'Alex Rivera'),
  ('work',     'Northwind', 0, 1, '127.0.0.1', 9, 'alex@northwind.example',  '', '127.0.0.1', 9, 'alex@northwind.example',  'demo', 'alex@northwind.example',  'Alex Rivera'),
  ('pebble',   'Pebble',    0, 1, '127.0.0.1', 9, 'alex@pebble.example',     '', '127.0.0.1', 9, 'alex@pebble.example',     'demo', 'alex@pebble.example',     'Alex from Pebble');
INSERT INTO folders SELECT id, '{"inbox":"INBOX","sent":"Sent","archive":"Archive","junk":"Junk","trash":"Trash","_all":["INBOX","Sent","Archive","Junk","Trash"]}' FROM email_accounts;

-- One row per email: `ago` in minutes, `facts` what the sync would have read in the body (codes, amounts, due dates).
CREATE TEMP TABLE m (account, folder, uid, ago, from_name, from_addr, to_addrs, subject, flags, category, thrid, snippet, body, facts DEFAULT '{}', list_unsub DEFAULT '');
INSERT INTO m (account, folder, uid, ago, from_name, from_addr, to_addrs, subject, flags, category, thrid, snippet, body, facts, list_unsub) VALUES
-- Personal
('personal','INBOX',101,  14,'Jamie Chen','jamie.chen@example.com','alex.rivera@example.com,priya.n@example.com,sam.okafor@example.com','Re: Cabin weekend — final plan','\Seen','personal','cabin',
 'Booked! The cabin is ours from Saturday 11:00. I''ll bring the board games, Priya has the groceries list.',
'Hi all,

Booked! The cabin at Pine Hollow is ours from Saturday 11:00 until Sunday 16:00. Check-in code arrives the morning of.

Here''s where we landed:

  • Driving: Sam picks up Priya at 9:00, I''ll swing by Alex''s at 9:15.
  • Food: Priya has the groceries list (shared note, add to it!). Alex is on breakfast.
  • Fun: I''ll bring the board games and the good speaker. Somebody pack the kayak paddles.

Total is $412 for the two nights, so $103 each. No rush, settle up whenever.

One more thing: the host says the trail to the lake is muddy after the rain, so bring boots.

Can''t wait,
Jamie','{}',''),
('personal','INBOX',100,  95,'Priya Natarajan','priya.n@example.com','alex.rivera@example.com,jamie.chen@example.com,sam.okafor@example.com','Re: Cabin weekend — final plan','\Seen','personal','cabin',
 'Groceries are on me. Alex, can you do pancakes again? Those were legendary.',
'Groceries are on me. Alex, can you do pancakes again? Those were legendary.

Priya','{}',''),
('personal','INBOX', 99, 260,'Sam Okafor','sam.okafor@example.com','alex.rivera@example.com,jamie.chen@example.com,priya.n@example.com','Cabin weekend — final plan','\Seen','personal','cabin',
 'Okay, it''s happening. Jamie, can you book the place we liked? I''ll drive.',
'Okay, it''s happening. Jamie, can you book the place we liked? I''ll drive.

Sam','{}',''),
('personal','INBOX', 98,  48,'Mom','rosa.rivera@example.net','alex.rivera@example.com','Sunday lunch?','','personal','',
 'Your aunt is visiting this weekend, come for lunch on Sunday if you''re back from the cabin in time.',
'Hi love,

Your aunt Lucía is visiting this weekend. Come for lunch on Sunday if you''re back from the cabin in time? I''m making the lemon chicken.

Besos,
Mom','{}',''),
('personal','INBOX', 97, 130,'Bluebird Bank','alerts@bluebird-bank.example','alex.rivera@example.com','Your verification code','\Seen','otp','',
 'Your Bluebird Bank code is 482 913. It expires in 10 minutes.',
'Your Bluebird Bank code is 482913. It expires in 10 minutes. Never share it with anyone.','{"code":"482913"}',''),
('personal','INBOX', 96, 310,'Fieldnotes Weekly','hello@fieldnotes.example','alex.rivera@example.com','Issue 142: The quiet joy of small tools','\Seen','newsletter','',
 'This week: why small, sharp tools beat platforms, a field guide to tea, and three essays worth your Sunday.',
'This week in Fieldnotes:

1. The quiet joy of small tools: why one sharp thing beats a platform.
2. A field guide to tea, for people who think they don''t like tea.
3. Three long reads for your Sunday.

Read online: https://fieldnotes.example/142','{}','<https://fieldnotes.example/unsubscribe>'),
('personal','INBOX', 95, 520,'Corner Grocer','orders@cornergrocer.example','alex.rivera@example.com','Your order #20417 is on its way','\Seen','receipt','',
 'Thanks, Alex! Your order of 14 items is on its way. Total charged: $68.40.',
'Thanks, Alex! Your order #20417 (14 items) is on its way and should arrive between 18:00 and 19:00.

Total charged: $68.40','{"amount":"$ 68.40"}',''),
('personal','INBOX', 94, 1500,'City Power & Light','billing@citypower.example','alex.rivera@example.com','Your November bill is ready','','receipt','',
 'Your bill for November is $84.12, due in 6 days. Pay online or set up autopay.',
'Hi Alex, your bill for November is ready.

Amount due: $84.12
Pay online at https://citypower.example/pay','{"amount":"$ 84.12"}',''),
('personal','INBOX', 93, 1620,'Trailhead Outfitters','news@trailhead.example','alex.rivera@example.com','Winter gear, 25% off this weekend','\Seen','marketing','',
 'Jackets, boots and everything for the cold months. This weekend only.',
'Jackets, boots and everything for the cold months, 25% off this weekend only.','{}','<https://trailhead.example/unsubscribe>'),
('personal','INBOX', 92, 2900,'Dr. Hana Sato''s office','frontdesk@smilecare.example','alex.rivera@example.com','Reminder: dental cleaning on Thursday','\Seen','calendar','',
 'This is a reminder of your appointment on Thursday at 08:00. Reply C to confirm.',
'This is a reminder of your dental cleaning on Thursday at 08:00 with Dr. Hana Sato. Reply C to confirm or call us to reschedule.','{}',''),
('personal','INBOX', 91, 4300,'Library Holds','holds@citylibrary.example','alex.rivera@example.com','"The Overstory" is ready for pickup','\Seen','notification','',
 'Your hold is ready at Central Branch. We''ll keep it for 7 days.',
'Your hold "The Overstory" is ready at Central Branch. We''ll keep it for 7 days.','{}',''),
('personal','Sent',  51, 200,'Alex Rivera','alex.rivera@example.com','sam.okafor@example.com,jamie.chen@example.com,priya.n@example.com','Re: Cabin weekend — final plan','\Seen','personal','',
 'I''m in! I''ll handle breakfast.', 'I''m in! I''ll handle breakfast.

Alex','{}',''),
('personal','Sent',  50, 3000,'Alex Rivera','alex.rivera@example.com','rosa.rivera@example.net','Photos from the weekend','\Seen','personal','',
 'Here are the photos from the park. The one with the dog is my favourite.', 'Here are the photos from the park. The one with the dog is my favourite.

Alex','{}',''),
('personal','Archive',31, 9000,'Skyway Airlines','trips@skyway.example','alex.rivera@example.com','Your trip to Lisbon: e-ticket','\Seen','receipt','',
 'Confirmation SKY7QX. Flight SW 482, seat 14A.', 'Confirmation SKY7QX. Flight SW 482, seat 14A.','{}',''),
-- Northwind (work)
('work','INBOX',201,  32,'Jordan Blake','jordan.blake@northwind.example','alex@northwind.example','Onboarding redesign: notes before Thursday','','personal','',
 'Read the draft over coffee. Two things I''d push on before the roadmap review: the empty states and step 3.',
'Hi Alex,

Read the draft over coffee. Overall it''s much clearer. Two things I''d push on before Thursday''s roadmap review:

1. The empty states still read like errors. Can we show an example project instead?
2. Step 3 asks for the team size before people know why. Move it after the first win?

Happy to pair on it tomorrow after our 1:1.

Jordan','{}',''),
('work','INBOX',200,  75,'Northwind CI','ci@northwind.example','alex@northwind.example','✓ main passed: web-app #1893','\Seen','notification','',
 'All 412 checks passed in 6m 12s. Deployed to staging.',
'All 412 checks passed in 6m 12s. Deployed to staging.','{}',''),
('work','INBOX',199, 180,'Maya Lindqvist','maya.l@northwind.example','alex@northwind.example','Invitation: Quarterly roadmap @ Thu 14:00','\Seen','calendar','',
 'Quarterly roadmap. Thursday 14:00 – 15:30, Room Atlas and video call.',
'You have been invited to: Quarterly roadmap
When: Thursday 14:00 – 15:30
Where: Room Atlas and video call','{}',''),
('work','INBOX',198, 340,'Theo Marsh','theo.marsh@northwind.example','alex@northwind.example','Figma file for the new settings page','\Seen \Flagged','personal','',
 'Here''s the file. I tried two layouts for the account section, curious which you prefer.',
'Here''s the file. I tried two layouts for the account section, curious which you prefer.

Theo','{}',''),
('work','INBOX',197, 600,'People Team','people@northwind.example','alex@northwind.example','Office closed on Friday the 5th','\Seen','notification','',
 'The office is closed for maintenance. Remote work as usual.',
'The office is closed on Friday the 5th for maintenance. Remote work as usual.','{}',''),
('work','INBOX',196, 1450,'Expensify Lite','receipts@expenses.example','alex@northwind.example','Expense approved: Team lunch','\Seen','receipt','',
 'Your expense "Team lunch" for $126.50 was approved by Maya Lindqvist.',
'Your expense "Team lunch" for $126.50 was approved by Maya Lindqvist.','{"amount":"$ 126.50"}',''),
('work','INBOX',195, 2800,'Product Weekly','digest@northwind.example','alex@northwind.example','Product Weekly: what shipped','\Seen','newsletter','',
 'Search got 40% faster, the mobile beta reached 2,000 testers, and a welcome to two new designers.',
'Search got 40% faster, the mobile beta reached 2,000 testers, and a welcome to two new designers.','{}','<https://northwind.example/digest/unsubscribe>'),
('work','INBOX',194, 3100,'Northwind SSO','no-reply@northwind.example','alex@northwind.example','Sign-in code','\Seen','otp','',
 'Your sign-in code is 771 204.','Your sign-in code is 771204.','{"code":"771204"}',''),
('work','Sent',  61, 60,'Alex Rivera','alex@northwind.example','maya.l@northwind.example','Re: Q1 hiring plan','\Seen','personal','',
 'Looks good. I''d add one more frontend role in February.','Looks good. I''d add one more frontend role in February.

Alex','{}',''),
('work','Archive',41, 8000,'Maya Lindqvist','maya.l@northwind.example','alex@northwind.example','Offsite agenda','\Seen','personal','',
 'Agenda for the offsite attached. Lunch is at the harbour.','Agenda for the offsite attached. Lunch is at the harbour.','{}',''),
-- Pebble (side project)
('pebble','INBOX',301,  22,'Lena Fischer','lena.fischer@example.org','alex@pebble.example','Love the app, one small request','','personal','',
 'Pebble replaced three apps for me. Any chance of a dark widget for the home screen?',
'Hi Alex,

Pebble replaced three apps for me, thank you for making it. Any chance of a dark widget for the home screen? The light one glows at night.

Lena','{}',''),
('pebble','INBOX',300, 150,'App Store Connect','no-reply@appstore.example','alex@pebble.example','Pebble 1.4 is ready for distribution','\Seen \Flagged','notification','',
 'The status of your app Pebble (1.4) is now Ready for Distribution.',
'The status of your app Pebble (1.4) is now Ready for Distribution.','{}',''),
('pebble','INBOX',299, 400,'Stripe','receipts@payments.example','alex@pebble.example','Payout of $1,284.00 is on the way','','receipt','',
 'Your payout of $1,284.00 should arrive in your bank account in 2 business days.',
'Your payout of $1,284.00 should arrive in your bank account in 2 business days.','{"amount":"$ 1,284.00"}',''),
('pebble','INBOX',298, 2200,'Indie Makers Digest','digest@indiemakers.example','alex@pebble.example','Pricing pages that don''t feel like pricing pages','\Seen','newsletter','',
 'Five pricing pages we loved this month, and what they have in common.',
'Five pricing pages we loved this month, and what they have in common.','{}','<https://indiemakers.example/unsubscribe>'),
('pebble','INBOX',297, 4100,'Ravi Kumar','ravi@kumar.example','alex@pebble.example','Re: Sync bug on iPad','\Seen','personal','',
 'Updated to the TestFlight build and the duplicate entries are gone. Thanks for the quick fix!',
'Updated to the TestFlight build and the duplicate entries are gone. Thanks for the quick fix!

Ravi','{}',''),
('pebble','Sent',  71, 30,'Alex from Pebble','alex@pebble.example','ravi@kumar.example','Re: Sync bug on iPad','\Seen','personal','',
 'Glad it''s fixed! 1.4 goes out to everyone on Friday.','Glad it''s fixed! 1.4 goes out to everyone on Friday.

Alex','{}','');

INSERT INTO msg (account, folder, uid, uidvalidity, msgid, thrid, from_name, from_addr, to_addrs, subject, ts, flags, list_unsub, category, extra)
  SELECT account, folder, uid, 1, '<' || account || '.' || uid || '@demo.example>', nullif(thrid, ''), from_name, from_addr, to_addrs, subject,
         unixepoch() - ago * 60, flags, list_unsub, category, json_patch(json_object('snippet', snippet), facts) FROM m;
-- What the reader shows when an email is opened (normally fetched over IMAP and kept here).
INSERT INTO mail_body (account, folder, uid, ts, data)
  SELECT account, folder, uid, unixepoch(), json_object('account', account, 'folder', folder, 'uid', uid, 'subject', subject,
         'from_name', from_name, 'from_address', from_addr, 'date', strftime('%Y-%m-%dT%H:%M:%SZ', unixepoch() - ago * 60, 'unixepoch'),
         'to', replace(to_addrs, ',', ', '), 'cc', '', 'body', body, 'body_html', '', 'message_id', '<' || account || '.' || uid || '@demo.example>',
         'references', '', 'attachments', json('[]')) FROM m;

-- Calendar: this week (Monday to Sunday), local times.
INSERT INTO calendars (id, name, color, source, read_only) VALUES
  ('cal-personal', 'Personal', '#5b8abf', 'local', 0),
  ('cal-work', 'Northwind', '#c46a3b', 'local', 0),
  ('cal-pebble', 'Pebble', '#4f9a6b', 'local', 0);
CREATE TEMP TABLE e (uid, cal, summary, day, start, mins, location DEFAULT '', rrule DEFAULT '', all_day DEFAULT 0);
INSERT INTO e (uid, cal, summary, day, start, mins, location, rrule, all_day) VALUES
  ('demo-standup',  'cal-work',     'Standup',                      0, '09:30', 15,  'Video call', 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR', 0),
  ('demo-planning', 'cal-work',     'Weekly planning',              0, '10:30', 60,  'Room Atlas', '', 0),
  ('demo-climb',    'cal-personal', 'Climbing with Sam',            0, '18:30', 90,  'Boulder Lab', '', 0),
  ('demo-review',   'cal-work',     'Design review: onboarding',    1, '11:00', 60,  'Video call', '', 0),
  ('demo-lunch',    'cal-personal', 'Lunch with Priya',             1, '13:00', 60,  'Café Lumière', '', 0),
  ('demo-oneone',   'cal-work',     '1:1 with Jordan',              2, '15:00', 30,  'Jordan''s desk', '', 0),
  ('demo-beta',     'cal-pebble',   'Pebble beta testers call',     2, '19:00', 45,  'Video call', '', 0),
  ('demo-dentist',  'cal-personal', 'Dentist',                      3, '08:00', 45,  'SmileCare, 12 Elm St', '', 0),
  ('demo-roadmap',  'cal-work',     'Quarterly roadmap',            3, '14:00', 90,  'Room Atlas', '', 0),
  ('demo-release',  'cal-pebble',   'Ship Pebble 1.4',              4, '12:00', 60,  '', '', 0),
  ('demo-dinner',   'cal-personal', 'Dinner at Mia''s',             4, '20:00', 150, '48 Harbor Rd', '', 0),
  ('demo-cabin',    'cal-personal', 'Cabin weekend',                5, '',      2880, 'Pine Hollow', '', 1),
  ('demo-gym',      'cal-personal', 'Gym',                          2, '07:15', 60,  'Northside Gym', '', 0);
INSERT INTO calendar_events (uid, calendar_id, summary, location, dtstart, dtend, all_day, is_utc, rrule, status, origin)
  SELECT uid, cal, summary, location,
         datetime(date('now', 'localtime', 'weekday 0', '-6 days', '+' || day || ' days') || ' ' || iif(all_day, '00:00', start)) || '.000000',
         datetime(date('now', 'localtime', 'weekday 0', '-6 days', '+' || day || ' days') || ' ' || iif(all_day, '00:00', start), '+' || mins || ' minutes') || '.000000',
         all_day, 0, rrule, 'confirmed', 'local' FROM e;

-- Notes and to-dos.
INSERT INTO notes (id, title, content, items, note_type, pinned, due_date, sort_order, created_at, updated_at) VALUES
  ('demo-n1', 'Cabin weekend packing', '', '[{"text":"Hiking boots","done":true},{"text":"Kayak paddles","done":false},{"text":"Pancake mix + maple syrup","done":false},{"text":"Board games","done":true},{"text":"Headlamp","done":false}]', 'note', 1, NULL, 0, datetime('now', '-2 days'), datetime('now', '-3 hours')),
  ('demo-n2', 'Pebble 1.4 release notes', 'New: home screen widgets in light and dark.
Faster sync on iPad, and no more duplicate entries.
Export your notes to Markdown.
Thanks to everyone in the beta!', '[]', 'note', 0, NULL, 1, datetime('now', '-5 days'), datetime('now', '-1 day')),
  ('demo-n3', 'Books to read', 'The Overstory (on hold at the library)
Piranesi
The Design of Everyday Things
A Psalm for the Wild-Built', '[]', 'note', 0, NULL, 2, datetime('now', '-20 days'), datetime('now', '-6 days')),
  ('demo-n4', 'Gift ideas for Mom', 'Lemon tree for the balcony, the cookbook from the market, concert tickets in spring.', '[]', 'note', 0, NULL, 3, datetime('now', '-9 days'), datetime('now', '-4 days')),
  ('demo-t1', 'Reply to Jordan about onboarding', '', '[]', 'todo', 0, date('now', 'localtime'), 0, datetime('now', '-1 day'), datetime('now', '-1 day')),
  ('demo-t2', 'Pay the electricity bill', '', '[]', 'todo', 0, date('now', 'localtime', '+2 days'), 1, datetime('now', '-1 day'), datetime('now', '-1 day')),
  ('demo-t3', 'Send Jamie $103 for the cabin', '', '[]', 'todo', 0, NULL, 2, datetime('now', '-1 hour'), datetime('now', '-1 hour'));

-- Dashboard: what this demo has data for (no news or market feeds).
INSERT INTO kv VALUES ('dash.layout', json('{"pages":[{"name":"Dashboard","columns":[
  {"size":"small","widgets":["calendar","weather-home"]},
  {"size":"full","widgets":["brief","todo"]},
  {"size":"small","widgets":["mail","clock"]}]}]}'));

-- Coding: one machine (offline here, it has no agentd) and two finished threads.
INSERT INTO code_machines (id, name, token_hash, main, created_at, last_seen, hello) VALUES ('laptop', 'laptop', 'demo-no-token', 1,
  (unixepoch() - 30 * 86400) * 1000, (unixepoch() - 600) * 1000, json('{"protocol":1,"machine":"laptop","version":"0.4.0","hostname":"laptop","os":"macos","arch":"aarch64",
  "home":"/Users/alex","agents":[{"agent":"claude-code","installed":true},{"agent":"codex","installed":true}],
  "projects":[{"path":"/Users/alex/code/pebble","name":"pebble","branch":"main"},{"path":"/Users/alex/code/northwind-web","name":"northwind-web","branch":"main"}],"threads":[]}'));
INSERT INTO code_threads (id, machine, agent, model, effort, permission, project, cwd, branch, title, state, unread, created_at, updated_at, preview, usage, cost, last_seq) VALUES
  ('t_demo000001', 'laptop', 'claude-code', 'opus', 'high', 'edits', '/Users/alex/code/pebble', '/Users/alex/code/pebble', 'dark-widget',
   'Dark home screen widget', 'idle', 1, (unixepoch() - 3000) * 1000, (unixepoch() - 600) * 1000,
   'Done: the widget follows the system appearance now, and the tests pass.', '{"input":18420,"output":2210,"cache_read":96000,"cache_write":12000}', 0.42, 11),
  ('t_demo000002', 'laptop', 'codex', 'gpt-5-codex', 'medium', 'ask', '/Users/alex/code/northwind-web', '/Users/alex/code/northwind-web', 'main',
   'Empty states for onboarding', 'idle', 0, (unixepoch() - 90000) * 1000, (unixepoch() - 86000) * 1000,
   'Replaced the three empty states with an example project card.', NULL, 0, 4);
CREATE TEMP TABLE ev (thread, seq, ago, data);
INSERT INTO ev VALUES
  ('t_demo000001', 1, 3000, '{"k":"user","text":"Lena (a beta tester) asked for a dark home screen widget: the light one glows at night. Can you make the Today widget follow the system appearance? Keep the accent color."}'),
  ('t_demo000001', 2, 2995, '{"k":"turn-started","turn":1}'),
  ('t_demo000001', 3, 2990, '{"k":"tool","item":"t1","name":"Read","kind":"read","title":"Read Widgets/TodayWidget.swift","input":{"file_path":"Widgets/TodayWidget.swift"}}'),
  ('t_demo000001', 4, 2989, '{"k":"tool-done","item":"t1","ok":true,"output":"struct TodayWidgetView: View {\n    let entry: TodayEntry\n    var body: some View {\n        VStack(alignment: .leading, spacing: 6) {\n            Text(entry.title).font(.headline)\n            ForEach(entry.items) { ItemRow(item: $0) }\n        }\n        .padding()\n        .background(Color.white)\n    }\n}"}'),
  ('t_demo000001', 5, 2980, '{"k":"message","item":"m1","text":"The widget paints its own `Color.white` background, so it ignores dark mode. I''ll switch it to the system container background, which follows the appearance."}'),
  ('t_demo000001', 6, 2970, '{"k":"tool","item":"t2","name":"Edit","kind":"edit","title":"Edit Widgets/TodayWidget.swift","input":{"file_path":"Widgets/TodayWidget.swift"}}'),
  ('t_demo000001', 7, 2969, '{"k":"tool-done","item":"t2","ok":true,"diffs":[{"path":"Widgets/TodayWidget.swift","unified":"@@ -5,6 +5,6 @@ struct TodayWidgetView: View {\n         VStack(alignment: .leading, spacing: 6) {\n             Text(entry.title).font(.headline)\n             ForEach(entry.items) { ItemRow(item: $0) }\n         }\n         .padding()\n-        .background(Color.white)\n+        .containerBackground(.fill.tertiary, for: .widget)"}]}'),
  ('t_demo000001', 8, 2950, '{"k":"tool","item":"t3","name":"Bash","kind":"exec","title":"swift test --filter WidgetTests","input":{"command":"swift test --filter WidgetTests"}}'),
  ('t_demo000001', 9, 2900, '{"k":"tool-done","item":"t3","ok":true,"output":"Test Suite ''WidgetTests'' started\nTest Case ''testLightAppearance'' passed (0.012 seconds)\nTest Case ''testDarkAppearance'' passed (0.011 seconds)\nTest Case ''testAccentColor'' passed (0.009 seconds)\nExecuted 3 tests, with 0 failures in 0.032 seconds"}'),
  ('t_demo000001', 10, 2890, '{"k":"message","item":"m2","text":"Done: the widget follows the system appearance now, and the tests pass.\n\n- **Background**: `containerBackground(.fill.tertiary, for: .widget)` instead of white, so iOS picks the light or dark material.\n- **Text and accent**: already semantic colors, so nothing else changes.\n\nWant me to add a dark preview to the widget gallery too?"}'),
  ('t_demo000001', 11, 2889, '{"k":"turn-done","turn":1,"stop":"end","usage":{"input":18420,"output":2210,"cache_read":96000,"cache_write":12000},"cost_usd":0.42,"duration_ms":111000}'),
  ('t_demo000002', 1, 90000, '{"k":"user","text":"Our onboarding empty states read like errors. Replace them with an example project card, like in Jordan''s notes."}'),
  ('t_demo000002', 2, 89990, '{"k":"turn-started","turn":1}'),
  ('t_demo000002', 3, 86100, '{"k":"message","item":"m1","text":"Replaced the three empty states with an example project card. The card opens a read-only sample project, and the \"Create your first project\" button stays as the main action."}'),
  ('t_demo000002', 4, 86000, '{"k":"turn-done","turn":1,"stop":"end","cost_usd":0.18,"duration_ms":240000}');
INSERT INTO code_events (thread, seq, ts, kind, data)
  SELECT thread, seq, (unixepoch() - ago) * 1000, data ->> '$.k', json_patch(data, json_object('seq', seq, 'ts', (unixepoch() - ago) * 1000)) FROM ev;
INSERT INTO code_fts (text, thread, seq) SELECT data ->> '$.text', thread, seq FROM ev WHERE data ->> '$.k' IN ('user', 'message');
