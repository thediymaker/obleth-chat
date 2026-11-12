"use client";

// Settings › MCP servers: servers that give the model extra tools in chats, the
// person's own and the ones administrators share with everyone.

import { Link2, Plus } from "lucide-react";
import Link from "next/link";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Note, Section } from "@/components/manage/kit";
import { SectionHeading } from "@/components/settings/settings-frame";
import { useCapabilities, useFeature } from "@/lib/queries";
import { useSession } from "@/lib/session";
import { ConnectionRow, useConnectionManager } from "./connection-dialogs";
import { kindOf, useConnections } from "./connections";

export function McpServersPage() {
  usePageChrome("MCP servers");
  const { isAdmin } = useSession();
  const capabilities = useCapabilities();
  const enabled = !!capabilities.data?.features?.connections;
  const clusters = useFeature("clusters");
  const { data, error, isLoading, refetch } = useConnections();
  const manager = useConnectionManager();
  const servers = (data ?? []).filter((c) => kindOf(c) === "mcp");
  const own = servers.filter((c) => !c.shared);
  const shared = servers.filter((c) => c.shared);
  if (capabilities.data && !enabled)
    return (
      <div>
        <SectionHeading title="MCP servers" />
        <EmptyState icon={Link2} title="Not set up on this server" text="MCP servers give the model extra tools in chats. An administrator turns connections on where Chat runs." />
      </div>
    );
  return (
    <div>
      {manager.element}
      <SectionHeading
        title="MCP servers"
        description={
          <>
            Servers that give the model extra tools in your chats. Tools that may change something ask you first.
            {clusters && (
              <>
                {" "}
                Slurm and Kubernetes clusters are on the{" "}
                <Link href="/clusters" className="underline underline-offset-2">
                  Clusters
                </Link>{" "}
                page.
              </>
            )}
          </>
        }
        actions={
          <Button size="sm" onClick={() => manager.add("mcp")} disabled={!enabled}>
            <Plus /> Add MCP server
          </Button>
        }
      />
      <ErrorBox error={error} onRetry={() => refetch()} />
      {isLoading && (
        <p role="status" className="mb-4 text-sm text-muted-foreground">
          Loading…
        </p>
      )}
      <div className="grid gap-6">
        <Section title="Yours">
          {own.length ? (
            <div className="grid gap-2">
              {own.map((c) => (
                <ConnectionRow key={c.id} c={c} manager={manager} />
              ))}
            </div>
          ) : (
            <EmptyState title="No MCP servers yet" text="Add one by its URL, such as https://example.com/mcp, with a token if it needs one." />
          )}
        </Section>
        {(shared.length > 0 || isAdmin) && (
          <Section
            title="Shared with everyone"
            description={isAdmin ? "Added by administrators for every account. Shared servers may use internal addresses." : "Added by your administrators. Turn one off to keep it out of your chats."}
          >
            {shared.length ? (
              <div className="grid gap-2">
                {shared.map((c) => (
                  <ConnectionRow key={c.id} c={c} manager={manager} />
                ))}
              </div>
            ) : (
              <Note>None yet.</Note>
            )}
          </Section>
        )}
      </div>
    </div>
  );
}
