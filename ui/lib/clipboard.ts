// Copying text: to the clipboard when the browser allows it, otherwise into a dialog
// that shows the text to copy by hand (components/app-shell/copy-dialog.tsx).

import { toast } from "sonner";

type ByHand = (text: string) => void;
let byHand: ByHand | null = null;

/** Where text goes when the clipboard is blocked; returns the undo. */
export function setCopyByHand(show: ByHand) {
  byHand = show;
  return () => {
    if (byHand === show) byHand = null;
  };
}

/** Copies text and says so, or shows it to copy by hand. */
export async function copyText(text: string, done = "Copied to clipboard") {
  try {
    await navigator.clipboard.writeText(text);
    toast(done);
  } catch {
    if (byHand) byHand(text);
    else toast("Copying is blocked here. Select the text and copy it instead.");
  }
}
