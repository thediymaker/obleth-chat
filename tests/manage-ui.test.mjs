import test from 'node:test';
import assert from 'node:assert/strict';
import { btn, iconButton, badge, displayName, icon } from '../web/manage-ui.js';

test('btn takes named options and escapes the label', () => {
  assert.equal(btn('A & B', 'go'), '<button type="button" class="btn " data-action="go" >A &amp; B</button>');
  assert.equal(btn('Retry', 'retry', { icon: 'refresh', tone: 'small', attrs: 'data-id="x"' }), `<button type="button" class="btn small" data-action="retry" data-id="x">${icon('refresh')}Retry</button>`);
});

test('btn can omit the type attribute for the chat chrome', () => {
  assert.equal(btn('Sign out', 'logout', { tone: 'primary', type: '' }), '<button class="btn primary" data-action="logout" >Sign out</button>');
  assert.match(btn('Save', 'save', { type: 'submit' }), /^<button type="submit" class="btn "/);
});

test('iconButton labels the control and badge escapes', () => {
  assert.equal(iconButton('x', 'close', 'Close "dialog"'), `<button class="icon-btn" data-action="close" aria-label="Close &quot;dialog&quot;" title="Close &quot;dialog&quot;" >${icon('x')}</button>`);
  assert.equal(badge('<b>', 'accent'), '<span class="pill accent">&lt;b&gt;</span>');
});

test('displayName prefers the display name and takes a fallback', () => {
  assert.equal(displayName({ displayName: 'Ada', username: 'ada' }), 'Ada');
  assert.equal(displayName({ display_name: 'Grace', username: 'grace' }), 'Grace');
  assert.equal(displayName({ username: 'linus' }), 'linus');
  assert.equal(displayName(null), 'Unknown account');
  assert.equal(displayName(null, ''), '');
});
