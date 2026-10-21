export const isPendingMessage = message => message?.role === 'assistant' && ['pending', 'streaming'].includes(message.status);
export const conversationBusy = conversation => !!conversation?.activeGenerationId || !!conversation?.messages?.some(isPendingMessage);
export const suggestionsPending = conversation => !!conversation?.messages?.some(message => message.followUpsStatus === 'pending');

// A checkpoint can arrive after a newer SSE chunk or the terminal event. Keep
// monotonic progress for the same message; never merge different versions.
export function mergeProgress(snapshot, current) {
  if (!current || snapshot.id !== current.id) return snapshot;
  const previous = new Map((current.messages || []).filter(message => message.id).map(message => [message.id, message]));
  return { ...snapshot, messages: (snapshot.messages || []).map(message => {
    const live = previous.get(message.id);
    if (!live || !isPendingMessage(message)) return message;
    if (!isPendingMessage(live)) return { ...message, ...live };
    return { ...message,
      content: (live.content?.length || 0) > (message.content?.length || 0) ? live.content : message.content,
      reasoningContent: (live.reasoningContent?.length || 0) > (message.reasoningContent?.length || 0) ? live.reasoningContent : message.reasoningContent,
    };
  }) };
}

export function responseVersion(message, direction) {
  const versions = message?.versions || [];
  const index = (message?.versionIndex || versions.findIndex(version => version.id === message?.id) + 1) - 1;
  return versions[index + direction]?.id || null;
}

// Everything a conversation render depends on except follow-up suggestions, which
// can be patched in place while the rest of the transcript stays untouched.
export function renderSignature(conversation) {
  const { messages = [], ...rest } = conversation || {};
  return JSON.stringify([rest, messages.map(({ followUps, followUpsStatus, ...message }) => message)]);
}
