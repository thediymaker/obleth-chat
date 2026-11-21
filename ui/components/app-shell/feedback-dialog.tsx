"use client";

// The sidebar's Feedback button opens this: a problem, an idea or anything else, sent
// to the site's administrators with the page it came from. Administration › Feedback
// lists what people sent; a name shows there only when they ticked the contact box.

import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { CheckField, Chips, Field, FormActions, FormDialog, FormError, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";

/** The keys are what the server stores (sent_feedback::KINDS). */
export const FEEDBACK_KINDS = [
  ["problem", "Something’s wrong"],
  ["idea", "An idea"],
  ["other", "Something else"],
] as const;
export type FeedbackKind = (typeof FEEDBACK_KINDS)[number][0];
export const FEEDBACK_LIMIT = 4000;

const PROMPTS: Record<FeedbackKind, { label: string; placeholder: string }> = {
  problem: { label: "What happened?", placeholder: "What you did, what you expected, and what happened instead" },
  idea: { label: "What would you like to see?", placeholder: "A feature, a change, or something that would make your work easier" },
  other: { label: "What’s on your mind?", placeholder: "Anything you want the people running this site to know" },
};

export function FeedbackDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const page = usePathname();
  const [kind, setKind] = useState<FeedbackKind>("problem");
  const [message, setMessage] = useState("");
  const [contact, setContact] = useState(false);
  const text = message.trim();
  const tooLong = text.length > FEEDBACK_LIMIT;
  const { busy, error, setError, onSubmit } = useSubmit(async () => {
    if (!text || tooLong) return;
    await api.request("/api/feedback", { method: "POST", body: { kind, message: text, contact, page } });
    setMessage("");
    setContact(false);
    onOpenChange(false);
    toast("Thanks, your feedback was sent");
  });
  // Each opening starts clean, except a draft that wasn't sent.
  useEffect(() => {
    if (open) setError("");
  }, [open, setError]);
  const prompt = PROMPTS[kind];
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Send feedback" description="Report a problem or suggest something you’d like to see. It goes to the people who run this site.">
      <form className="grid gap-4" onSubmit={onSubmit}>
        <Chips label="Kind of feedback" options={FEEDBACK_KINDS} value={kind} onChange={setKind} />
        <Field id="feedback-message" label={prompt.label}>
          <Textarea id="feedback-message" rows={6} autoFocus value={message} placeholder={prompt.placeholder} onChange={(e) => setMessage(e.target.value)} aria-invalid={tooLong || undefined} />
          {tooLong && <p className="text-xs text-destructive-foreground">Up to {FEEDBACK_LIMIT.toLocaleString()} characters.</p>}
        </Field>
        <CheckField id="feedback-contact" label="You can contact me about this" description="Your name is shared with this feedback only if you tick this." checked={contact} onCheckedChange={setContact} />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={!text || tooLong || busy}>
            {busy ? "Sending…" : "Send"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
