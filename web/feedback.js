import { esc, iconButton } from './manage-ui.js';

// Why a reply was not good; the keys are what the server stores (feedback::REASONS).
export const REASONS = [['wrong', 'Wrong or made up'], ['instructions', 'Didn’t follow instructions'], ['too_long', 'Too long'], ['too_short', 'Too short'], ['harmful', 'Harmful or offensive'], ['other', 'Other']];
export const COMMENT_LIMIT = 2000;
export const reasonLabel = key => REASONS.find(([k]) => k === key)?.[1] || key;

/** Whether the person may rate this reply: a finished reply in their own chat. */
export const ratable = (m, shared) => !shared && !!m?.id && m.role === 'assistant' && ['completed', 'interrupted'].includes(m.status);

/** Clicking the thumb that is already chosen clears it; the other one switches. */
export const nextRating = (current, clicked) => current === clicked ? 0 : clicked;

/** Thumbs up and down for the reply's action row. */
export function feedbackButtons(m, index, shared) {
  if (!ratable(m, shared)) return '';
  const rating = m.feedback?.rating || 0;
  return `${iconButton('thumbsUp', 'rate-up', 'Good response', `data-index="${index}" aria-pressed="${rating === 1}"`)}${iconButton('thumbsDown', 'rate-down', 'Bad response', `data-index="${index}" aria-pressed="${rating === -1}"`)}`;
}

/** The optional details for a thumbs down: reasons and a comment. */
export function feedbackForm(m, index) {
  const chosen = new Set(m.feedback?.reasons || []);
  return `<form id="feedback-form" data-index="${index}"><p class="inline-note">Thanks. If you like, say what went wrong. Only you and administrators see this, and administrators don’t see the conversation or who sent it.</p><fieldset class="feedback-reasons"><legend class="form-label">What was wrong?</legend>${REASONS.map(([key, label]) => `<label class="tag-chip"><input type="checkbox" name="reason" value="${key}" ${chosen.has(key) ? 'checked' : ''}>${esc(label)}</label>`).join('')}</fieldset><label class="form-label" for="feedback-comment">Comment <small class="faint">(optional)</small></label><textarea id="feedback-comment" name="comment" class="form-control" rows="3" maxlength="${COMMENT_LIMIT}" placeholder="What would have been better?">${esc(m.feedback?.comment || '')}</textarea><div class="modal-actions"><button type="button" class="btn" data-action="close">Skip</button><button type="submit" class="btn primary">Send feedback</button></div></form>`;
}

/** The rating the thumbs-down form sends. */
export function feedbackDetails(form) {
  const reasons = [...form.querySelectorAll('input[name="reason"]:checked')].map(input => input.value);
  const comment = (form.querySelector('[name="comment"]')?.value || '').trim().slice(0, COMMENT_LIMIT);
  return { rating: -1, reasons, ...(comment ? { comment } : {}) };
}
