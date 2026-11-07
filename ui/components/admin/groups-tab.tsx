"use client";

// Administration › Groups: the groups, and for one group its members, adding and
// removing people, editing and deleting it.

import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft, Plus, Users } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Field, FormActions, FormDialog, FormError, Dropdown, Note, Section, TableWrap, TextAreaField, TextField, displayName, td, th, titleCase, useConfirm, useSubmit } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { SearchForm, useAdminUsers } from "./people-tab";
import { useKept } from "./state";

export interface AdminGroup {
  id: string;
  name: string;
  description?: string;
  memberCount?: number;
}
interface Member {
  id: string;
  username: string;
  displayName?: string;
  display_name?: string;
  role: string;
  enabled?: boolean;
  managedOidc?: boolean;
}

export const groupsKey = ["admin", "groups"] as const;
export function useAdminGroups(enabled = true) {
  return useQuery({ queryKey: groupsKey, queryFn: async () => (await api.request<{ groups?: AdminGroup[] }>("/api/admin/groups")).groups ?? [], enabled });
}

function GroupDialog({ group, onClose, onSaved }: { group: AdminGroup | null; onClose: () => void; onSaved: () => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const id = group?.id;
    await api.request(`/api/admin/groups${id ? `/${encodeURIComponent(id)}` : ""}`, {
      method: id ? "PATCH" : "POST",
      body: { name: String(form.get("group-name") || "").trim(), description: String(form.get("group-description") || "").trim() },
    });
    onSaved();
    toast(id ? "Group updated" : "Group created");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={group ? "Edit group" : "Create group"}>
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="group-name" label="Group name" maxLength={100} required defaultValue={group?.name || ""} />
        <TextAreaField id="group-description" label="Description" maxLength={1000} rows={3} defaultValue={group?.description || ""} />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            {group ? "Save group" : "Create group"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function GroupsTab({ groupId }: { groupId: string }) {
  return groupId ? <GroupDetail id={groupId} /> : <GroupList />;
}

function GroupList() {
  const queries = useQueryClient();
  const { data: groups = [], error, isLoading, refetch } = useAdminGroups();
  const [creating, setCreating] = useState(false);
  const create = (
    <Button size="sm" onClick={() => setCreating(true)}>
      <Plus /> New group
    </Button>
  );
  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Note className="max-w-2xl">Members share projects and collections with their group, and a model can be limited to certain groups. Groups do not grant administrator access.</Note>
        {create}
      </div>
      <ErrorBox error={error} onRetry={() => refetch()} />
      {isLoading ? (
        <Loading label="Loading administration…" />
      ) : groups.length ? (
        <TableWrap>
          <table className="w-full min-w-[520px]">
            <thead className="border-b bg-muted/40">
              <tr>
                <th className={th}>Group</th>
                <th className={th}>Members</th>
                <th className={th}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {groups.map((g) => (
                <tr key={g.id} className="border-b border-line-soft last:border-0">
                  <td className={td}>
                    <div className="flex items-center gap-2.5">
                      <span aria-hidden className="grid size-8 shrink-0 place-items-center rounded-full bg-group text-group-foreground">
                        <Users className="size-4" />
                      </span>
                      <div className="grid min-w-0">
                        <Link href={`/settings/admin/groups/${encodeURIComponent(g.id)}`} className="truncate font-medium hover:underline">
                          {g.name}
                        </Link>
                        <small className="truncate text-xs text-muted-foreground">{g.description || "No description yet."}</small>
                      </div>
                    </div>
                  </td>
                  <td className={td}>
                    {Number(g.memberCount || 0)} member{Number(g.memberCount) === 1 ? "" : "s"}
                  </td>
                  <td className={`${td} text-right`}>
                    <Button size="xs" variant="outline" asChild>
                      <Link href={`/settings/admin/groups/${encodeURIComponent(g.id)}`}>Manage</Link>
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <EmptyState
          title="Start with a group"
          text="Organize a class, a project team, or a set of people with the same model access."
          action={
            <Button size="sm" onClick={() => setCreating(true)}>
              <Plus /> Create a group
            </Button>
          }
        />
      )}
      {creating && (
        <GroupDialog
          group={null}
          onClose={() => setCreating(false)}
          onSaved={() => {
            setCreating(false);
            queries.invalidateQueries({ queryKey: groupsKey });
          }}
        />
      )}
    </div>
  );
}

function GroupDetail({ id }: { id: string }) {
  const queries = useQueryClient();
  const router = useRouter();
  const confirm = useConfirm();
  const [query, setQuery] = useKept("groups.memberQuery", "");
  const [editing, setEditing] = useState(false);
  const key = ["admin", "group", id] as const;
  const detail = useInfiniteQuery({
    queryKey: key,
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      api.request<{ group: AdminGroup; members?: Member[]; nextCursor?: string | null }>(`/api/admin/groups/${encodeURIComponent(id)}${pageParam ? `?after=${encodeURIComponent(pageParam)}` : ""}`),
    getNextPageParam: (last) => last.nextCursor || undefined,
  });
  const users = useAdminUsers(query);
  const group = detail.data?.pages[0]?.group;
  const members: Member[] = [];
  for (const page of detail.data?.pages ?? []) for (const m of page.members ?? []) if (!members.some((old) => old.id === m.id)) members.push(m);
  const candidates = (users.data?.users ?? []).filter((u) => u.enabled !== false && !members.some((m) => m.id === u.id));
  const reload = () => {
    queries.invalidateQueries({ queryKey: key });
    queries.invalidateQueries({ queryKey: groupsKey });
  };
  const add = useSubmit(async (form) => {
    const user = String(form.get("member-user") || "");
    if (!user) return;
    await api.request(`/api/admin/groups/${encodeURIComponent(id)}/members/${encodeURIComponent(user)}`, { method: "PUT" });
    reload();
    toast("Member added");
  });
  if (!group)
    return (
      <div className="grid gap-4">
        <BackLink />
        <ErrorBox error={detail.error} onRetry={() => detail.refetch()} />
        {!detail.error && <Loading label="Loading administration…" />}
      </div>
    );
  return (
    <div className="grid gap-4">
      {confirm.element}
      <BackLink />
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold">{group.name}</h2>
          <Note>{group.description || "No description yet."}</Note>
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
            Edit group
          </Button>
          <Button
            size="sm"
            variant="destructive"
            onClick={() =>
              confirm.ask({
                title: "Delete this group?",
                text: "Memberships will be removed. Models restricted to this group may become inaccessible to its members.",
                label: "Delete group",
                action: async () => {
                  await api.request(`/api/admin/groups/${encodeURIComponent(id)}`, { method: "DELETE" });
                  queries.invalidateQueries({ queryKey: groupsKey });
                  router.push("/settings/admin/groups");
                  toast("Group deleted");
                },
              })
            }
          >
            Delete
          </Button>
        </div>
      </div>
      <ErrorBox error={detail.error || users.error} onRetry={() => detail.refetch()} />
      <Section title="Add a member" headingLevel={3}>
        <SearchForm id="admin-member-query" label="Find an account" placeholder="Search name or username…" value={query} onSearch={setQuery} />
        <form onSubmit={add.onSubmit} className="flex flex-wrap items-end gap-2">
          <Field id="member-user" label="Account" className="min-w-0 grow sm:max-w-md">
            <Dropdown id="member-user" name="member-user" disabled={!candidates.length}>
              {candidates.length ? (
                candidates.map((u) => (
                  <option key={u.id} value={u.id}>
                    {displayName(u)} · {u.username}
                  </option>
                ))
              ) : (
                <option value="">No matching accounts to add</option>
              )}
            </Dropdown>
          </Field>
          <Button type="submit" size="sm" disabled={!candidates.length || add.busy}>
            Add member
          </Button>
        </form>
        <FormError message={add.error} />
        {users.data?.hasMore && <Note>Refine your search to find accounts beyond the first 1,000 results.</Note>}
      </Section>
      {members.length ? (
        <TableWrap>
          <table className="w-full min-w-[480px]">
            <thead className="border-b bg-muted/40">
              <tr>
                <th className={th}>Member</th>
                <th className={th}>Role</th>
                <th className={th}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {members.map((u) => (
                <tr key={u.id} className="border-b border-line-soft last:border-0">
                  <td className={td}>
                    <strong className="block font-medium">{displayName(u)}</strong>
                    <small className="text-xs text-muted-foreground">
                      {u.username}
                      {u.managedOidc ? " · From single sign-on" : ""}
                    </small>
                  </td>
                  <td className={td}>{titleCase(u.role)}</td>
                  <td className={`${td} text-right`}>
                    <Button
                      size="xs"
                      variant="outline"
                      onClick={() =>
                        confirm.ask({
                          title: "Remove this member?",
                          text: "This person will lose access granted through this group.",
                          label: "Remove member",
                          action: async () => {
                            await api.request(`/api/admin/groups/${encodeURIComponent(id)}/members/${encodeURIComponent(u.id)}`, { method: "DELETE" });
                            reload();
                            toast("Member removed");
                          },
                        })
                      }
                    >
                      Remove
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <EmptyState title="No members yet" text="Add accounts above to grant the model access configured for this group." />
      )}
      {detail.hasNextPage && (
        <div className="flex justify-center">
          <Button size="sm" variant="outline" disabled={detail.isFetchingNextPage} onClick={() => detail.fetchNextPage()}>
            Load more members
          </Button>
        </div>
      )}
      {editing && (
        <GroupDialog
          group={group}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false);
            reload();
          }}
        />
      )}
    </div>
  );
}

function BackLink() {
  return (
    <Link href="/settings/admin/groups" className="inline-flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
      <ChevronLeft className="size-4" /> All groups
    </Link>
  );
}
