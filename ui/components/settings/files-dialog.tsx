"use client";

// Your files: the private attachments stored in the account, with Attach (to the
// next message in Chat) and Delete. Opened from Settings › Your files.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, File, FileText, Image as ImageIcon, Monitor, Sheet } from "lucide-react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { EmptyState } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { FormActions, FormDialog, Note, useConfirm } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";

export interface Attachment {
  id: string;
  name: string;
  kind: "file" | "document" | "image" | string;
  size: number;
  parts?: number;
  searched?: boolean;
  createdAt?: string;
}
const sizeLabel = (n: number) =>
  n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : n < 1024 ** 3 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${(n / 1024 ** 3).toFixed(1)} GB`;
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
    return { Icon: type ? Archive : File, detail: [type || extension(a.name).toUpperCase(), entries, sizeLabel(a.size)].filter(Boolean).join(" · ") };
  }
  const doc = a.kind === "document" ? DOCUMENTS[extension(a.name)] : undefined;
  if (!doc) return { Icon: a.kind === "image" ? ImageIcon : File, detail: sizeLabel(a.size) };
  const [type, Icon, unit] = doc;
  const parts = unit && a.parts ? `${a.parts.toLocaleString()} ${unit}${a.parts === 1 ? "" : "s"}` : "";
  return { Icon, detail: [type, parts, sizeLabel(a.size)].filter(Boolean).join(" · ") };
}

const filesKey = ["settings", "files"] as const;

export function FilesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const queries = useQueryClient();
  const router = useRouter();
  const confirm = useConfirm();
  const { data, error, isLoading } = useQuery({
    queryKey: filesKey,
    queryFn: async () => (await api.request<{ attachments?: Attachment[] }>("/api/attachments")).attachments ?? [],
    enabled: open,
  });
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Your files" wide>
      {confirm.element}
      <Note>
        Files are private to your account. Attach any file to a conversation: Chat reads text, code and documents (PDF, Word, PowerPoint, Excel), shows images to models that can see them, and puts every file in the conversation’s
        workspace when it has one.
      </Note>
      {isLoading ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading files…
        </p>
      ) : error ? (
        <Note role="alert">{error.message}</Note>
      ) : data?.length ? (
        <ul className="grid max-h-[50dvh] gap-1 overflow-y-auto">
          {data.map((a) => {
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
        </ul>
      ) : (
        <EmptyState title="No uploaded files" text="Use the paperclip in Chat to attach a file." />
      )}
      <FormActions>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Done
        </Button>
      </FormActions>
    </FormDialog>
  );
}
