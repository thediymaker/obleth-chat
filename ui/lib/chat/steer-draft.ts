// What the person typed in Steer and has not sent, kept while the reply's card
// re-renders. It belongs to whoever is signed in: the chat store clears it when
// their session ends, with the other drafts.
export const steerDraft = { text: "" };
export const resetSteerDraft = () => {
  steerDraft.text = "";
};
