"use client";

// Your files: the private attachments stored in the account, with Attach (to the
// next message in Chat) and Delete. Opened from Settings › Your files.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Archive, File, FileText, Image as ImageIcon, Monitor, Sheet } from "lucide-react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { FormActions, FormDialog, Note, useConfirm } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when, bytes } from "@/lib/format";

export interface Attachment {
  id: string;
  name: string;
  kind: "file" | "document" | "image" | string;
  size: number;
  parts?: number;
  searched?: boolean;
  createdAt?: string;
}
const DOCUMENTS: Record<string, [string, typeof File, string]> = { pdf: ["PDF", FileText, "page"], docx: ["Word", FileText, ""], pptx: ["PowerPoint", Monitor, "slide"], xlsx: ["Excel", Sheet, "sheet"] };
const extension = (name: string) => (name.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
const ARCHIVES = [
  [".tar.gz", "TAR.GZ"],
  [".tgz", "TGZ"],
  [".tar.bz2", "TAR.BZ2"],
  [".tar.xz", "TAR.XZ"],
  [".tar", "TAR"],
  [".zip", "ZIP"],
  [".7z", "7Z"],
  [".rar", "RAR"],
  [".gz", "GZ"],
  [".xz", "XZ"],
  [".bz2", "BZ2"],
];
/** The icon and the line under a file's name: "PDF · 12 pages · 1.2 MB" (attachments.js describe). */
export function describeFile(a: Attachment): { Icon: typeof File; detail: string } {
  if (a.kind === "file") {
    const type = ARCHIVES.find(([end]) => a.name.toLowerCase().endsWith(end!))?.[1];
    const entries = type && a.parts ? `${a.parts.toLocaleString()} file${a.parts === 1 ? "" : "s"}` : "";
    return { Icon: type ? Archive : File, detail: [type || extension(a.name).toUpperCase(), entries, bytes(a.size)].filter(Boolean).join(" · ") };
  }
  const doc = a.kind === "document" ? DOCUMENTS[extension(a.name)] : undefined;
  if (!doc) return { Icon: a.kind === "image" ? ImageIcon : File, detail: bytes(a.size) };
  const [type, Icon, unit] = doc;
  const parts = unit && a.parts ? `${a.parts.toLocaleString()} ${unit}${a.parts === 1 ? "" : "s"}` : "";
  return { Icon, detail: [type, parts, bytes(a.size)].filter(Boolean).join(" · ") };
}

const filesKey = ["settings", "files"] as const;
/** Files shown at first; "Show more" asks for another page, so the oldest can be reached. */
export const FILES_PAGE = 100;

export function FilesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const queries = useQueryClient();
  const router = useRouter();
  const confirm = useConfirm();
  const [shown, setShown] = useState(FILES_PAGE);
  const { data, error, isLoading } = useQuery({
    queryKey: [...filesKey, shown],
    queryFn: () => api.request<{ items?: Attachment[]; more?: boolean }>(`/api/attachments?limit=${shown}`),
    enabled: open,
  });
  const files = data?.items ?? [];
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Your files" wide>
      {confirm.element}
      <Note>
        Files are private to your account. Attach any file to a conversation: Chat reads text, code and documents (PDF, Word, PowerPoint, Excel), shows images to models that can see them, and puts every file in the conversation’s
        workspace when it has one.
      </Note>
      {isLoading ? (
        <Loading compact label="Loading files…" />
      ) : error ? (
        <ErrorBox compact error={error} />
      ) : files.length ? (
        <ul className="grid max-h-[50dvh] gap-1 overflow-y-auto">
          {files.map((a) => {
            const { Icon, detail } = describeFile(a);
            return (
              <li key={a.id} className="flex items-center gap-3 rounded-md border px-3 py-2">
                <Icon className="size-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 grow">
                  <h3 className="truncate text-md font-medium">{a.name}</h3>
                  <p className="text-xs text-muted-foreground">
                    {detail}
                    {a.createdAt ? ` · ${when(a.createdAt)}` : ""}
                  </p>
                </div>
                <Button
                  size="xs"
                  variant="outline"
                  onClick={() => {
                    onOpenChange(false);
                    router.push(`/?attach=${encodeURIComponent(a.id)}`);
                    toast("File attached to your next message");
                  }}
                >
                  Attach
                </Button>
                <Button
                  size="xs"
                  variant="outline"
                  onClick={() =>
                    confirm.ask({
                      title: "Delete this file?",
                      text: "Files referenced by saved conversations cannot be deleted.",
                      label: "Delete file",
                      cancel: "Keep file",
                      action: async () => {
                        await api.request(`/api/attachments/${encodeURIComponent(a.id)}`, { method: "DELETE" });
                        await queries.invalidateQueries({ queryKey: filesKey });
                        toast("File deleted");
                      },
                    })
                  }
                >
                  Delete
                </Button>
              </li>
            );
          })}
          {data?.more && (
            <li>
              <Button variant="ghost" size="sm" onClick={() => setShown(shown + FILES_PAGE)}>
                Show more files
              </Button>
            </li>
          )}
        </ul>
      ) : (
        <EmptyState title="No uploaded files" text="Use the paperclip in Chat to attach a file." />
      )}
      <FormActions cancel={false}>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Done
        </Button>
      </FormActions>
    </FormDialog>
  );
}
