"use client";

import { useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { chatDraft } from "@/components/projects/chat-draft";
import { Confirm, FormError, FormField, messageOf } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { count } from "@/lib/format";
import { useSession } from "@/lib/session";
import { type CollectionDetail, collectionPath, knowledgeKeys, refreshKnowledge, type Scope, useCollections } from "./api";

/** New collection, or Edit with Delete. A new one opens its page, or goes to `onCreated`
 * when given, as from a project, with `name` to start from. */
export function CollectionDialog({
  open,
  onOpenChange,
  detail,
  name: startName = "",
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  detail?: CollectionDetail;
  name?: string;
  onCreated?: (id: string) => void | Promise<void>;
}) {
  const router = useRouter();
  const client = useQueryClient();
  const { isAdmin } = useSession();
  const { data } = useCollections(open);
  const groups = data?.groups ?? [];
  const c = detail?.collection;
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [scope, setScope] = useState<Scope>("personal");
  const [group, setGroup] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName(c?.name ?? startName);
    setDescription(c?.description ?? "");
    setScope(c?.scope ?? "personal");
    setGroup("");
    setError("");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fresh each time it opens
  }, [open]);
  useEffect(() => {
    if (!group && groups[0]) setGroup(groups[0].id);
  }, [group, groups]);

  const options: [Scope, string][] = [
    ["personal", "Only you"],
    ...(groups.length || scope === "group" ? ([["group", "A group"]] as [Scope, string][]) : []),
    ...(isAdmin || scope === "everyone" ? ([["everyone", "Everyone"]] as [Scope, string][]) : []),
  ];

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const body: Record<string, unknown> = { name: name.trim(), description: description.trim() };
    if (!c) {
      body.scope = scope;
      if (scope === "group") body.groupId = group;
    }
    try {
      const r = await api.request<{ collection: { id: string } }>(c ? collectionPath(c.id) : "/api/knowledge/collections", { method: c ? "PATCH" : "POST", body });
      onOpenChange(false);
      refreshKnowledge(client, c?.id);
      if (c) toast("Collection saved");
      else if (onCreated) await onCreated(r.collection.id);
      else {
        router.push(`/knowledge/${encodeURIComponent(r.collection.id)}`);
        toast("Collection created. Add documents next.");
      }
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{c ? `Edit ${c.name}` : "Make space for your knowledge"}</DialogTitle>
            <DialogDescription className="sr-only">A collection holds documents chats can search.</DialogDescription>
          </DialogHeader>
          <form id="knowledge-form" className="grid gap-4" onSubmit={submit}>
            <FormField id="knowledge-name" label="Name">
              <Input id="knowledge-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} required placeholder="e.g. PHYS 241 · Thermodynamics" autoFocus />
            </FormField>
            <FormField id="knowledge-description" label="A little context">
              <Textarea id="knowledge-description" rows={3} maxLength={500} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What belongs here?" />
            </FormField>
            {!c && (
              <FormField id="knowledge-scope-new" label="Who can search it">
                <Select value={scope} onValueChange={(v) => setScope(v as Scope)}>
                  <SelectTrigger id="knowledge-scope-new" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {options.map(([v, l]) => (
                      <SelectItem key={v} value={v}>
                        {l}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FormField>
            )}
            {!c && groups.length > 0 && scope === "group" && (
              <FormField id="knowledge-group-new" label="Group" help="Used when sharing with a group.">
                <Select value={group} onValueChange={setGroup}>
                  <SelectTrigger id="knowledge-group-new" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {groups.map((g) => (
                      <SelectItem key={g.id} value={g.id}>
                        {g.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FormField>
            )}
            <FormError message={error} />
            {c && (
              <div className="flex items-center gap-3 rounded-md border border-destructive/30 p-3">
                <div className="grow">
                  <strong className="block text-md font-medium">Delete this collection</strong>
                  <small className="text-xs text-muted-foreground">Removes its documents and their files for everyone.</small>
                </div>
                <Button type="button" size="sm" variant="outline" className="text-destructive-foreground" onClick={() => setDeleting(true)}>
                  <Trash2 /> Delete
                </Button>
              </div>
            )}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy}>
                {c ? "Save" : "Create collection"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      {c && (
        <Confirm
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete ${c.name}?`}
          text={`Its ${count(detail!.documents.length, "document")} and their files are removed for everyone, and chats stop searching it. This cannot be undone.`}
          cancel="Keep it"
          confirm="Delete collection"
          onConfirm={async () => {
            await api.request(collectionPath(c.id), { method: "DELETE" });
            onOpenChange(false);
            chatDraft.setCollections(chatDraft.get().collections.filter((id) => id !== c.id));
            client.removeQueries({ queryKey: knowledgeKeys.detail(c.id) });
            refreshKnowledge(client);
            router.push("/knowledge");
            toast("Collection deleted");
          }}
        />
      )}
    </>
  );
}
