import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeBranding } from '../web/branding.js';
import { authScreen } from '../web/sign-in.js';

const config = normalizeBranding({
  siteName: 'Research\nComputing',
  login: {
    title: 'Sign in to Chat',
    description: 'Models <run> here.',
    ssoLabel: 'Continue with Voyager',
    ssoHint: 'Uses your campus sign-in.',
    help: 'No account yet?',
    helpLink: { label: 'Request one', url: 'https://accounts.example.edu' },
  },
});

test('password-only sign-in makes the password button primary and escapes configured copy', () => {
  const html = authScreen({ phase: 'login', oidc: false }, config);
  assert.match(html, /<h1>Sign in to Chat<\/h1>/);
  assert.match(html, /Models &lt;run&gt; here\./);
  assert.doesNotMatch(html, /oidc\/login|Continue with Voyager|or sign in with a password/);
  assert.match(html, /<button class="btn primary" type="submit" >Sign in<\/button>/);
  assert.match(html, /autocomplete="username" maxlength="128" required autofocus/);
  assert.match(html, /No account yet\? <a href="https:\/\/accounts\.example\.edu" target="_blank" rel="noopener noreferrer">Request one<\/a>/);
  assert.match(html, /id="auth-theme"/);
});

test('single sign-on leads and passwords come second', () => {
  const html = authScreen({ phase: 'login', oidc: true }, config);
  assert.ok(html.indexOf('/api/auth/oidc/login') < html.indexOf('id="login-form"'));
  assert.match(html, /<a class="btn primary" href="\/api\/auth\/oidc\/login">.*Continue with Voyager<\/a>/);
  assert.match(html, /Uses your campus sign-in\./);
  assert.match(html, /<button class="btn" type="submit" >Sign in with password<\/button>/);
  assert.doesNotMatch(html, /autofocus/);
});

test('with passwords turned off only the single sign-on button is offered', () => {
  const html = authScreen({ phase: 'login', oidc: true, passwords: false }, config);
  assert.match(html, /<a class="btn primary" href="\/api\/auth\/oidc\/login">/);
  assert.doesNotMatch(html, /login-form|type="password"|or sign in with a password/);
  // Without single sign-on there is nothing else to offer, so the form stays.
  assert.match(authScreen({ phase: 'login', oidc: false, passwords: false }, config), /id="login-form"/);
});

test('a blank title names the site, and a session notice is not shown as an error', () => {
  const html = authScreen({ phase: 'login', loginNotice: 'Your session has expired.' }, normalizeBranding({ siteName: 'Acme <Chat>' }));
  assert.match(html, /<h1>Sign in to Acme &lt;Chat&gt;<\/h1>/);
  assert.match(html, /class="auth-notice" role="status">Your session has expired\./);
  assert.doesNotMatch(html, /auth-error/);
  assert.match(authScreen({ phase: 'login', loginError: 'Invalid username or password' }, config), /class="auth-error" role="alert">.*Invalid username or password/);
});

test('every signed-out phase keeps the form ids and actions the app listens for', () => {
  assert.match(authScreen({ phase: 'activate' }, config), /id="activation-form".*id="activation-password".*id="activation-confirm"/);
  const required = authScreen({ phase: 'password-required', user: { username: 'pat"' } }, config);
  assert.match(required, /id="required-password-form".*value="pat&quot;".*id="required-current".*id="required-password".*id="required-confirm".*data-action="logout"/);
  assert.match(authScreen({ phase: 'loading' }, config), /role="status">.*Checking your sign-in…/);
  const failed = authScreen({ phase: 'error', error: 'The request failed (502).' }, config);
  assert.match(failed, /Can’t connect.*class="auth-detail">The request failed \(502\)\.<\/p>.*data-action="retry-session"/);
});

test('a banner shown to signed-out visitors sits above the sign-in page', () => {
  assert.doesNotMatch(authScreen({ phase: 'login', oidc: false }, config), /site-banner/);
  const html = authScreen({ phase: 'login', oidc: false, banner: { text: 'In development', tone: 'warning' } }, config);
  assert.match(html, /^<main id="main" class="auth-layout"><div class="site-banner warning">.*In development<\/span><\/div><header class="auth-header">/);
});
