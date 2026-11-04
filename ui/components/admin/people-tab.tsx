"use client";

// Administration › People: accounts with their role, status and last sign-in;
// creating accounts and invitations, one-time links, password resets, single
// sign-on identities, signing someone out, and turning an account off.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link2, Plus, Search } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  CheckField,
  CopyField,
  DetailRow,
  FormActions,
  FormDialog,
  FormError,
  Note,
  Pill,
  SelectField,
  TableWrap,
  TextField,
  displayName,
  td,
  th,
  titleCase,
  useConfirm,
  useSubmit,
} from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useSession } from "@/lib/session";
import { useKept } from "./state";

export interface AdminUser {
  id: string;
  username: string;
  display_name?: string;
  displayName?: string;
  role: "admin" | "user";
  enabled?: boolean;
  last_sign_in_at?: string | null;
}
export interface AccessLink {
  kind: "invite" | "reset";
  url: string;
  expiresAt: string;
}
const ROLES = [
  ["user", "User"],
  ["admin", "Administrator"],
] as const;

export const usersKey = (query: string) => ["admin", "users", query] as const;
export function useAdminUsers(query: string) {
  return useQuery({
    queryKey: usersKey(query),
    queryFn: () => api.request<{ users?: AdminUser[]; hasMore?: boolean }>(`/api/admin/users${query ? `?q=${encodeURIComponent(query)}` : ""}`),
  });
}

/** A search box that applies on submit. */
export function SearchForm({ id, label, placeholder, value, onSearch, maxLength = 128 }: { id: string; label: string; placeholder: string; value: string; onSearch: (q: string) => void; maxLength?: number }) {
  return (
    <form
      role="search"
      className="flex min-w-0 grow items-center gap-2 sm:max-w-md"
      onSubmit={(e) => {
        e.preventDefault();
        onSearch(String(new FormData(e.currentTarget).get("query") || "").trim());
      }}
    >
      <label className="sr-only" htmlFor={id}>
        {label}
      </label>
      <Input key={value} id={id} name="query" placeholder={placeholder} maxLength={maxLength} defaultValue={value} />
      <Button type="submit" size="sm" variant="outline">
        <Search /> Search
      </Button>
    </form>
  );
}

export function AccessLinkDialog({ link, onClose }: { link: AccessLink | null; onClose: () => void }) {
  return (
    <FormDialog open={!!link} onOpenChange={(o) => !o && onClose()} title={link?.kind === "invite" ? "Invitation ready" : "Password reset link ready"}>
      {link && (
        <>
          <Note>Share this link privately. It can be used once and expires {when(link.expiresAt)}.</Note>
          <CopyField id="one-time-link" label="One-time link" value={link.url} rows={3} copyLabel="Copy link" done="One-time link copied" />
          <FormActions>
            <Button variant="outline" onClick={onClose}>
              Done
            </Button>
          </FormActions>
        </>
      )}
    </FormDialog>
  );
}

function AccountDialog({ invite, onClose, onDone }: { invite: boolean; onClose: () => void; onDone: (link: AccessLink | null) => void }) {
  const { passwords } = useSession();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const body: Record<string, unknown> = { username: String(form.get("account-username") || "").trim(), displayName: String(form.get("account-name") || "").trim(), role: String(form.get("account-role")) };
    if (!invite && passwords !== false) {
      body.password = String(form.get("account-password") || "");
      body.forcePasswordChange = form.get("account-force-change") === "on";
    }
    const r = await api.request<AccessLink>(invite ? "/api/admin/invitations" : "/api/admin/users", { method: "POST", body });
    onDone(invite ? r : null);
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={invite ? "Invite a person" : "Create an account"}>
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="account-username" label="Username" maxLength={128} autoComplete="off" required />
        <TextField id="account-name" label="Display name" maxLength={128} required />
        <SelectField id="account-role" label="Role" options={ROLES} defaultValue="user" help="Administrators manage accounts and model access." />
        {invite ? (
          <Note>A one-time link lets this person choose their password. You can copy it after creating the account.</Note>
        ) : passwords === false ? (
          <Note>Passwords are off. Link the account to single sign-on after creating it.</Note>
        ) : (
          <>
            <TextField id="account-password" label="Initial password" type="password" minLength={12} autoComplete="new-password" required />
            <CheckField id="account-force-change" label="Require a password change at first sign-in" defaultChecked />
          </>
        )}
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            {invite ? "Create invitation" : "Create account"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function ResetPasswordDialog({ user, onClose }: { user: AdminUser; onClose: () => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    await api.request(`/api/admin/users/${encodeURIComponent(user.id)}/password`, {
      method: "POST",
      body: { newPassword: String(form.get("temporary-password") || ""), forcePasswordChange: form.get("temporary-force-change") === "on" },
    });
    onClose();
    toast("Password reset; existing sessions signed out");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Set a temporary password">
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="temporary-password" label="New password" type="password" minLength={12} autoComplete="new-password" required />
        <CheckField id="temporary-force-change" label="Require a password change at next sign-in" defaultChecked />
        <Note>Existing sessions will be signed out.</Note>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Reset password
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function LinkOidcDialog({ user, onClose }: { user: AdminUser; onClose: () => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    await api.request("/api/admin/oidc-identities", { method: "POST", body: { userId: user.id, subject: String(form.get("oidc-subject") || "").trim() } });
    onClose();
    toast("Single sign-on identity linked");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Link single sign-on">
      <form onSubmit={onSubmit} className="grid gap-4">
        <Note>Use the exact, verified subject identifier for this person from your configured identity provider.</Note>
        <TextField id="oidc-subject" label="Provider subject identifier" maxLength={1024} autoComplete="off" required />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Link identity
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

type Dialog = null | { type: "create" | "invite" } | { type: "manage" | "reset" | "oidc"; user: AdminUser };

export function PeopleTab() {
  const session = useSession();
  const queries = useQueryClient();
  const [query, setQuery] = useKept("users.query", "");
  const { data, error, isLoading, refetch } = useAdminUsers(query);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [link, setLink] = useState<AccessLink | null>(null);
  const confirm = useConfirm();
  const users = data?.users ?? [];
  const reload = () => queries.invalidateQueries({ queryKey: ["admin", "users"] });
  /** After a change to one's own account the session is read again; otherwise the list. */
  const changed = async (u: AdminUser) => {
    if (u.id === session.user?.id) await session.reload();
    await reload();
  };
  const manage = dialog?.type === "manage" ? dialog.user : null;

  const editForm = useSubmit(async (form) => {
    if (!manage) return;
    const body = { displayName: String(form.get("managed-name") || "").trim(), role: String(form.get("managed-role")) };
    const save = async () => {
      await api.request(`/api/admin/users/${encodeURIComponent(manage.id)}`, { method: "PATCH", body });
      setDialog(null);
      await changed(manage);
      toast("Account updated");
    };
    if (body.role !== manage.role)
      confirm.ask({
        title: "Change this account’s role?",
        text:
          body.role === "admin"
            ? "This person will be able to manage users, groups, model access, and installation settings."
            : "This person will lose administrator access. Their personal conversations will stay private and available.",
        label: "Change role",
        action: save,
      });
    else await save();
  });

  return (
    <div className="grid gap-4">
      {confirm.element}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <SearchForm id="admin-users-query" label="Search accounts" placeholder="Search name or username…" value={query} onSearch={setQuery} />
        <div className="flex flex-wrap gap-2">
          {session.passwords !== false && (
            <Button size="sm" variant="outline" onClick={() => setDialog({ type: "invite" })}>
              <Link2 /> Invite person
            </Button>
          )}
          <Button size="sm" onClick={() => setDialog({ type: "create" })}>
            <Plus /> Create account
          </Button>
        </div>
      </div>
      {data?.hasMore && <Note>Showing the first 1,000 matches. Refine the search to find another account.</Note>}
      <ErrorBox error={error} onRetry={() => refetch()} />
      {isLoading ? (
        <Loading label="Loading administration…" />
      ) : users.length ? (
        <TableWrap>
          <table className="w-full min-w-[640px]">
            <thead className="border-b bg-muted/40">
              <tr>
                <th className={th}>Person</th>
                <th className={th}>Role</th>
                <th className={th}>Status</th>
                <th className={th}>Last sign-in</th>
                <th className={th}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {users.map((u) => (
                <tr key={u.id} className="border-b border-line-soft last:border-0">
                  <td className={td}>
                    <div className="flex items-center gap-2.5">
                      <span aria-hidden className="grid size-8 shrink-0 place-items-center rounded-full bg-secondary text-2xs font-semibold">
                        {displayName(u).slice(0, 2).toUpperCase()}
                      </span>
                      <div className="grid min-w-0">
                        <strong className="flex items-center gap-1.5 truncate font-medium">
                          {displayName(u)} {u.id === session.user?.id && <Pill>You</Pill>}
                        </strong>
                        <small className="truncate text-xs text-muted-foreground">{u.username}</small>
                      </div>
                    </div>
                  </td>
                  <td className={td}>
                    <Pill tone={u.role === "admin" ? "accent" : ""}>{titleCase(u.role)}</Pill>
                  </td>
                  <td className={td}>
                    <Pill tone={u.enabled === false ? "" : "accent"}>{u.enabled === false ? "Disabled" : "Active"}</Pill>
                  </td>
                  <td className={`${td} text-faint`}>{u.last_sign_in_at ? <time dateTime={u.last_sign_in_at}>{when(u.last_sign_in_at)}</time> : "Never"}</td>
                  <td className={`${td} text-right`}>
                    <Button size="xs" variant="outline" onClick={() => setDialog({ type: "manage", user: u })}>
                      Manage
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <EmptyState title="No accounts found" text="Try another search, or create an account for someone joining your workspace." />
      )}

      {(dialog?.type === "create" || dialog?.type === "invite") && (
        <AccountDialog
          invite={dialog.type === "invite"}
          onClose={() => setDialog(null)}
          onDone={async (made) => {
            setDialog(null);
            await reload();
            if (made) setLink(made);
            else toast("Account created");
          }}
        />
      )}
      <AccessLinkDialog link={link} onClose={() => setLink(null)} />
      {manage && (
        <FormDialog open onOpenChange={(o) => !o && setDialog(null)} title={`Manage ${displayName(manage)}`}>
          <form onSubmit={editForm.onSubmit} className="grid gap-4">
            <TextField id="managed-name" label="Display name" maxLength={100} required defaultValue={displayName(manage)} />
            <SelectField id="managed-role" label="Role" options={ROLES} defaultValue={manage.role} />
            <div>
              <DetailRow label="Username">{manage.username}</DetailRow>
              <DetailRow label="Access">{manage.enabled === false ? "Disabled" : "Active"}</DetailRow>
            </div>
            <FormError message={editForm.error} />
            <FormActions>
              <Button type="submit" disabled={editForm.busy}>
                Save changes
              </Button>
            </FormActions>
          </form>
          <div className="grid gap-2 border-t pt-4">
            <h3 className="text-md font-medium">Account security</h3>
            <div className="flex flex-wrap gap-2">
              {session.passwords !== false && (
                <>
                  <Button size="xs" variant="outline" onClick={() => setDialog({ type: "reset", user: manage })}>
                    Reset password
                  </Button>
                  <Button
                    size="xs"
                    variant="outline"
                    onClick={async () => {
                      try {
                        const r = await api.request<AccessLink>(`/api/admin/users/${encodeURIComponent(manage.id)}/access-link`, { method: "POST", body: { kind: "reset", expiresInHours: 24 } });
                        setDialog(null);
                        setLink(r);
                      } catch (e) {
                        toast((e as Error).message);
                      }
                    }}
                  >
                    Create reset link
                  </Button>
                </>
              )}
              {session.oidc && (
                <Button size="xs" variant="outline" onClick={() => setDialog({ type: "oidc", user: manage })}>
                  Link single sign-on
                </Button>
              )}
              <Button
                size="xs"
                variant="outline"
                onClick={() =>
                  confirm.ask({
                    title: "Sign out this person?",
                    text: "All their active sessions will be revoked. They can sign in again with their password or single sign-on.",
                    label: "Revoke sessions",
                    action: async () => {
                      await api.request(`/api/admin/users/${encodeURIComponent(manage.id)}/revoke-sessions`, { method: "POST" });
                      setDialog(null);
                      toast("Sessions revoked");
                    },
                  })
                }
              >
                Revoke sessions
              </Button>
              <Button
                size="xs"
                variant={manage.enabled === false ? "outline" : "destructive"}
                onClick={() => {
                  const enabled = manage.enabled === false;
                  confirm.ask({
                    title: enabled ? "Enable this account?" : "Disable this account?",
                    text: enabled ? "This person will be able to sign in again." : "This person will be signed out and unable to sign in until the account is enabled again.",
                    label: enabled ? "Enable account" : "Disable account",
                    tone: enabled ? "default" : "danger",
                    action: async () => {
                      await api.request(`/api/admin/users/${encodeURIComponent(manage.id)}`, { method: "PATCH", body: { enabled } });
                      setDialog(null);
                      await changed(manage);
                      toast(enabled ? "Account enabled" : "Account disabled");
                    },
                  });
                }}
              >
                {manage.enabled === false ? "Enable account" : "Disable account"}
              </Button>
            </div>
          </div>
        </FormDialog>
      )}
      {dialog?.type === "reset" && <ResetPasswordDialog user={dialog.user} onClose={() => setDialog(null)} />}
      {dialog?.type === "oidc" && <LinkOidcDialog user={dialog.user} onClose={() => setDialog(null)} />}
    </div>
  );
}
