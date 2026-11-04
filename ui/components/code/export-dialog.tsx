"use client";

// Taking a project's code out: a zip with its history and
// chats if wanted, or a push to GitHub; and bringing it in from a repository
// (importDialog, repoFields, importRepo), which the new-project form can use too.

import { useQueryClient } from "@tanstack/react-query";
import { Download, ExternalLink, Globe, Upload } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { CheckField, FormActions, FormDialog, FormError, Note, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { api } from "@/lib/api";
import { type CodeOverview, count, projectCodePath, slug } from "./model";
import { startDownload } from "@/components/workspace/model";

export const codeKeys = {
  all: (projectId: string) => ["project-code", projectId] as const,
};

/** The zip's address, with or without its history and chats. */
export const zipUrl = (projectId: string, history: boolean, chats: boolean) => `${projectCodePath(projectId)}/export.zip?history=${history}&chats=${chats}`;

function Part({ icon: Icon, title, text, children }: { icon: typeof Download; title: string; text: string; children?: React.ReactNode }) {
  return (
    <section className="grid gap-3 rounded-lg border p-4">
      <div>
        <h3 className="flex items-center gap-2 text-md font-medium">
          <Icon className="size-4 text-muted-foreground" />
          {title}
        </h3>
        <Note className="mt-1">{text}</Note>
      </div>
      {children}
    </section>
  );
}

export function ExportDialog({ projectId, name, overview, open, onOpenChange }: { projectId: string; name: string; overview?: CodeOverview | null; open: boolean; onOpenChange: (open: boolean) => void }) {
  const queries = useQueryClient();
  const last = overview?.exports?.find((e) => e.kind === "github");
  const [owner, repo] = last ? last.target.split("/") : ["", slug(name)];
  const [history, setHistory] = useState(true);
  const [chats, setChats] = useState(false);
  const [pushed, setPushed] = useState<{ url: string; target: string } | null>(null);
  const [visibility, setVisibility] = useState("private");
  const push = useSubmit(async (data) => {
    const body = {
      owner: String(data.get("code-gh-owner") || "").trim() || undefined,
      name: String(data.get("code-gh-name") || "").trim(),
      private: visibility === "private",
      token: String(data.get("code-gh-token") || "").trim(),
    };
    const r = await api.request<{ url: string; target: string }>(`${projectCodePath(projectId)}/github`, { method: "POST", body });
    setPushed(r);
    queries.invalidateQueries({ queryKey: codeKeys.all(projectId) });
  });
  const close = (o: boolean) => {
    onOpenChange(o);
    if (!o) setPushed(null);
  };
  if (pushed)
    return (
      <FormDialog open={open} onOpenChange={close} title="Pushed to GitHub">
        <p className="text-md">
          The code and its history are at{" "}
          <a href={pushed.url} target="_blank" rel="noopener" className="underline underline-offset-2">
            {pushed.target}
          </a>
          . Push again from the project any time to update it.
        </p>
        <Note>To put a site online, import that repository in Vercel or Netlify, or turn on GitHub Pages in its settings.</Note>
        <FormActions>
          <Button asChild variant="outline">
            <a href={pushed.url} target="_blank" rel="noopener">
              <ExternalLink /> Open on GitHub
            </a>
          </Button>
          <Button onClick={() => close(false)}>Done</Button>
        </FormActions>
      </FormDialog>
    );
  return (
    <FormDialog open={open} onOpenChange={close} title="Take the code with you" description="The code, its history, and if you like the chats that made it. Export as often as you want; each push to the same repository updates it." wide>
      <div className="grid gap-3">
        <Part icon={Download} title="Download a zip" text="The code as it is on main, for a class hand-in, an archive, or another host.">
          <CheckField
            id="code-zip-history"
            checked={history}
            onCheckedChange={setHistory}
            label="Its history"
            description={
              <>
                As a git bundle: <code className="font-mono">git clone {slug(name)}.bundle</code> gets every commit back.
              </>
            }
          />
          <CheckField
            id="code-zip-chats"
            checked={chats}
            onCheckedChange={setChats}
            label="Your chats in the project"
            description={
              <>
                As Markdown in <code className="font-mono">chats/</code>, without tool steps.
              </>
            }
          />
          <div>
            <Button size="sm" variant="outline" onClick={() => startDownload(zipUrl(projectId, history, chats))}>
              <Download /> Download zip
            </Button>
          </div>
        </Part>
        <Part icon={Upload} title="Push to GitHub" text="Every commit goes along. The repository is created if it doesn’t exist yet.">
          <form className="grid gap-3" onSubmit={push.onSubmit}>
            <div className="grid grid-cols-[1fr_auto_1fr] items-end gap-2">
              <TextField id="code-gh-owner" label="Owner" defaultValue={owner} placeholder="Your account" maxLength={39} autoComplete="off" spellCheck={false} />
              <span className="pb-2 text-muted-foreground">/</span>
              <TextField id="code-gh-name" label="Repository" defaultValue={repo} required maxLength={100} autoComplete="off" spellCheck={false} />
            </div>
            <div className="grid gap-1.5">
              <Label>Visibility</Label>
              <RadioGroup value={visibility} onValueChange={setVisibility} className="flex gap-4">
                <label className="flex items-center gap-2 text-md">
                  <RadioGroupItem value="private" /> Private
                </label>
                <label className="flex items-center gap-2 text-md">
                  <RadioGroupItem value="public" /> Public
                </label>
              </RadioGroup>
            </div>
            <TextField
              id="code-gh-token"
              label="GitHub token"
              type="password"
              required
              autoComplete="off"
              spellCheck={false}
              placeholder="github_pat_…"
              help="Used for this push only and never kept. A fine-grained token needs Contents: read and write, plus Administration: read and write to create the repository."
            />
            <FormError message={push.error} />
            <FormActions>
              <Button type="submit" disabled={push.busy}>
                <Upload /> {push.busy ? "Pushing…" : "Push to GitHub"}
              </Button>
            </FormActions>
          </form>
        </Part>
        <Part icon={Globe} title="Put a site online" text="Push to GitHub first. Then import the repository in Vercel or Netlify, or turn on GitHub Pages in its settings. Each later push redeploys it." />
      </div>
    </FormDialog>
  );
}

/** The repository address and token fields, named code-repo-url and code-repo-token. */
export function RepoFields({ optional = false }: { optional?: boolean }) {
  return (
    <div className="grid gap-2">
      <TextField
        id="code-repo-url"
        label={optional ? "Start from a repository (optional)" : "Repository"}
        required={!optional}
        type="url"
        maxLength={300}
        autoComplete="off"
        spellCheck={false}
        placeholder="https://github.com/owner/name"
        help="Chat copies its default branch once, with its history, as the project’s code."
      />
      <Collapsible>
        <CollapsibleTrigger className="text-sm text-muted-foreground hover:text-foreground">Private repository?</CollapsibleTrigger>
        <CollapsibleContent className="pt-2">
          <TextField id="code-repo-token" label="Access token" type="password" autoComplete="off" spellCheck={false} placeholder="github_pat_…" help="Used for this copy only and never kept. It needs read access to the repository’s contents." />
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}

/** Fills an empty project's code from a repository; throws with the reason it can't. */
export async function importRepo(projectId: string, url: string, token: string) {
  const r = await api.request<{ history: boolean; commits: number }>(`${projectCodePath(projectId)}/import`, { method: "POST", body: { url, ...(token ? { token } : {}) } });
  toast(r.history ? `Copied ${count(r.commits, "commit")} from the repository` : "Copied the repository’s latest version; its history was too big to keep");
  return r;
}

export function ImportDialog({ projectId, open, onOpenChange }: { projectId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const queries = useQueryClient();
  const submit = useSubmit(async (data) => {
    await importRepo(projectId, String(data.get("code-repo-url") || "").trim(), String(data.get("code-repo-token") || "").trim());
    await queries.invalidateQueries({ queryKey: codeKeys.all(projectId) });
    onOpenChange(false);
  });
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Start from a repository" description="The repository’s history becomes the project’s, so Sync, History, Undo and research build on it. Its chats’ workspaces start with the code.">
      <form className="grid gap-3" onSubmit={submit.onSubmit}>
        <RepoFields />
        <FormError message={submit.error} />
        <FormActions>
          <Button type="submit" disabled={submit.busy}>
            <Download /> {submit.busy ? "Copying…" : "Copy the repository"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
