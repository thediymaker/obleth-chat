import { branding, brand, siteFooter, copy } from './branding.js';
import { btn, esc, icon } from './manage-ui.js';
import { themeControl } from './theme-control.js';

// The signed-out screens: sign in, an invitation, a required password change,
// the session check and its failure. One quiet column, the same in every phase.

const field = (id, label, attrs, help = '') => `<label class="form-label" for="${id}">${label}</label><input class="form-control" id="${id}" ${attrs}${help ? ` aria-describedby="${id}-help"` : ''}>${help ? `<p class="auth-field-help" id="${id}-help">${help}</p>` : ''}`;
const newPassword = (id, label, help = '') => field(id, label, 'type="password" minlength="12" autocomplete="new-password" required', help);
const intro = (title, text = '') => `<div class="auth-intro"><h1>${title}</h1>${text ? `<p>${text}</p>` : ''}</div>`;
const external = url => /^(https?:|mailto:)/i.test(url) ? ' target="_blank" rel="noopener noreferrer"' : '';

function helpLine(login) {
  const link = login.helpLink ? `<a href="${esc(login.helpLink.url)}"${external(login.helpLink.url)}>${esc(login.helpLink.label)}</a>` : '';
  const text = [copy(login.help), link].filter(Boolean).join(' ');
  return text ? `<p class="auth-help">${text}</p>` : '';
}

function signIn(state, config) {
  const login = config.login;
  // With single sign-on the provider is the main way in and passwords come second.
  const sso = state.oidc ? `<a class="btn primary" href="/api/auth/oidc/login">${icon('signin')}${esc(login.ssoLabel)}</a>${login.ssoHint ? `<p class="auth-hint">${copy(login.ssoHint)}</p>` : ''}<div class="auth-divider">or sign in with a password</div>` : '';
  const notice = state.loginNotice ? `<p class="auth-notice" role="status">${esc(state.loginNotice)}</p>` : '';
  const error = state.loginError ? `<div class="auth-error" role="alert">${icon('info')}<span>${esc(state.loginError)}</span></div>` : '';
  const label = state.busy ? 'Signing in…' : state.oidc ? 'Sign in with password' : 'Sign in';
  return `${intro(login.title ? copy(login.title) : `Sign in to ${esc(config.siteName)}`, copy(login.description))}${notice}${error}${sso}<form id="login-form">${field('username', 'Username', `name="username" autocomplete="username" maxlength="128" required${state.oidc ? '' : ' autofocus'}`)}${field('password', 'Password', 'name="password" type="password" autocomplete="current-password" required')}<button class="btn${state.oidc ? '' : ' primary'}" type="submit" ${state.busy ? 'disabled' : ''}>${label}</button></form>${helpLine(login)}`;
}

const phases = {
  activate: () => `${intro('Set your password', 'Choose the password you’ll use to sign in.')}<form id="activation-form">${newPassword('activation-password', 'New password', 'At least 12 characters.')}${newPassword('activation-confirm', 'Type it again')}<div class="form-error" role="alert"></div><button type="submit" class="btn primary">Save and sign in</button></form>`,
  'password-required': state => `${intro('Choose a new password', 'Your administrator asked for a new password before you continue.')}<form id="required-password-form"><input type="hidden" autocomplete="username" value="${esc(state.user?.username)}">${field('required-current', 'Current password', 'type="password" autocomplete="current-password" required')}${newPassword('required-password', 'New password', 'At least 12 characters.')}${newPassword('required-confirm', 'Type it again')}<div class="form-error" role="alert"></div><button type="submit" class="btn primary">Update password</button></form>${btn('Sign out', 'logout', { tone: 'ghost small auth-sign-out' })}`,
  loading: () => '<div class="auth-status" role="status"><span class="auth-spinner" aria-hidden="true"></span><p>Checking your sign-in…</p></div>',
  error: state => `${intro('Can’t connect', 'We couldn’t check your sign-in. Try again in a minute.')}${state.error ? `<p class="auth-detail">${esc(state.error)}</p>` : ''}${btn('Try again', 'retry-session', { icon: 'refresh', tone: 'primary' })}`,
};

export function authScreen(state, config = branding) {
  const body = (phases[state.phase] || (s => signIn(s, config)))(state);
  return `<main id="main" class="auth-layout"><header class="auth-header">${brand(config)}${themeControl('auth-theme')}</header><section class="auth-panel"><div class="auth-card">${body}</div></section>${siteFooter(config)}</main>`;
}
