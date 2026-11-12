"use client";

// Compute › Devices: boards and instruments plugged into the person's computer,
// reached through this browser over USB serial. Each is a connection of kind
// "device" whose tools (open, read, send, reset, identify, flash) the model uses in
// chats; this page adds them, attaches a port to them in this tab, and shows a
// serial monitor for an attached one.

import { useQueryClient } from "@tanstack/react-query";
import { Cpu, KeyRound, Plus, Unplug, Usb } from "lucide-react";
import { useEffect, useReducer, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Eyebrow, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { useChatStore } from "@/components/chat/chat-provider";
import { connectionsKey, isDevice, useConnections, type Connection } from "@/components/connections/connections";
import { CheckField, FormActions, FormDialog, FormError, Note, Pill, SelectField, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { saveSecret, secretsKey, useSecrets } from "@/components/settings/secrets-page";
import { Button } from "@/components/ui/button";
import { ComputeOff, ComputeTabs } from "@/components/workspace/compute";
import { SerialTerminal } from "./serial-terminal";
import { api } from "@/lib/api";
import { withFlasher } from "@/lib/devices/flashers";
import { portLabel, suggestName } from "@/lib/devices/known";
import { DEFAULT_BAUD } from "@/lib/devices/serial";
import { useCapabilities } from "@/lib/queries";
import { cn } from "@/lib/utils";

const BAUDS = [9600, 19200, 38400, 57600, 74880, 115200, 230400, 460800, 921600];
const ids = (c: { vendorId?: number | null; productId?: number | null }) => (c.vendorId != null ? `USB ${c.vendorId.toString(16).padStart(4, "0")}:${(c.productId ?? 0).toString(16).padStart(4, "0")}` : "");

/** Re-renders when the devices bridge changes (attach, output, state). */
function useDevices() {
  const store = useChatStore();
  const [, bump] = useReducer((n: number) => n + 1, 0);
  useEffect(() => store.devices.subscribe(bump), [store]);
  return store.devices;
}

export interface Picked {
  port: SerialPort;
  vendorId?: number;
  productId?: number;
}

/** The dialog after the person picked a port: name it, and whether changes ask first. */
export function AddDeviceDialog({ picked, suggested, onClose, onAdded }: { picked: Picked | null; suggested?: string; onClose: () => void; onAdded?: (item: Connection) => void }) {
  const queries = useQueryClient();
  const devices = useDevices();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    if (!picked) return;
    const options = { transport: "serial", vendorId: picked.vendorId, productId: picked.productId, label: portLabel(picked.vendorId, picked.productId) };
    const item = await api.request<Connection>("/api/connections", { method: "POST", body: { kind: "device", name: String(form.get("device-name") || "").trim(), url: "", confirmActions: form.get("device-confirm") === "on", options } });
    devices.adopt({ id: item.id, name: item.name, vendorId: picked.vendorId, productId: picked.productId }, picked.port);
    await queries.invalidateQueries({ queryKey: connectionsKey });
    toast(`${item.name} added. Its tools are on in your chats.`);
    if (onAdded) onAdded(item);
    else onClose();
  });
  return (
    <FormDialog open={!!picked} onOpenChange={(o) => !o && onClose()} title="Add a device" description={picked ? `Port picked: ${portLabel(picked.vendorId, picked.productId)}.` : undefined}>
      <form className="grid gap-4" onSubmit={onSubmit}>
        <TextField id="device-name" label="Name" required maxLength={40} autoComplete="off" defaultValue={suggested || (picked ? suggestName(picked.vendorId, picked.productId) : "")} help="How chats and the model refer to it, such as “ESP32-S3 devkit”." />
        <CheckField id="device-confirm" label="Ask before changing calls" description="Sending, resetting, flashing and erasing wait for your Allow. Reading never asks." defaultChecked />
        <Note>The device is reached only from a browser tab where you attached it: this one, now. A tab on another computer asks you to attach it again.</Note>
        <FormError message={error} />
        <FormActions>
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            Add device
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

/** The device's console sign-in: a user name, and the secret holding the password. */
function SignInDialog({ device, onClose }: { device: Connection | null; onClose: () => void }) {
  const queries = useQueryClient();
  const devices = useDevices();
  const secrets = useSecrets().data ?? [];
  const [choice, setChoice] = useState<string>("");
  const chosen = choice || device?.secret || (secrets[0]?.name ?? "new");
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    if (!device) return;
    const user = String(form.get("signin-user") || "").trim();
    let secret = String(form.get("signin-secret") || "");
    if (secret === "new") {
      const name = String(form.get("signin-secret-name") || "").trim();
      const value = String(form.get("signin-secret-value") || "");
      secret = (await saveSecret(name, value, `${device.name} console password`)).name;
      await queries.invalidateQueries({ queryKey: secretsKey });
    }
    const item = await api.request<Connection>(`/api/connections/${encodeURIComponent(device.id)}`, { method: "PATCH", body: { options: { user, secret } } });
    devices.update({ id: item.id, name: item.name, user: item.user || undefined, secret: item.secret || undefined });
    await queries.invalidateQueries({ queryKey: connectionsKey });
    toast(`${item.name} signs in as ${user}`);
    onClose();
  });
  const options: [string, string][] = [...secrets.map((s) => [s.name, s.note ? `${s.name} · ${s.note}` : s.name] as [string, string]), ["new", "New secret…"]];
  return (
    <FormDialog open={!!device} onOpenChange={(o) => !o && onClose()} title="Console sign-in" description="What the browser types at the device's login prompt when the model calls Sign in at the console. The model never sees the password.">
      <form className="grid gap-4" onSubmit={onSubmit}>
        <TextField id="signin-user" label="User name" required maxLength={63} autoComplete="off" defaultValue={device?.user || ""} placeholder="debian" />
        <SelectField id="signin-secret" label="Password" value={chosen} onChange={(e) => setChoice(e.target.value)} options={options} help="A secret from Settings › Secrets, or a new one." />
        {chosen === "new" && (
          <>
            <TextField id="signin-secret-name" label="Secret name" required maxLength={40} autoComplete="off" pattern="[A-Za-z0-9][A-Za-z0-9_.\-]*" defaultValue={device ? `${device.name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")}-password` : ""} />
            <TextField id="signin-secret-value" label="Secret value" type="password" required autoComplete="new-password" maxLength={4096} help="Stored encrypted and never shown again." />
          </>
        )}
        <FormError message={error} />
        <FormActions>
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function EditDialog({ device, onClose }: { device: Connection | null; onClose: () => void }) {
  const queries = useQueryClient();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    if (!device) return;
    await api.request(`/api/connections/${encodeURIComponent(device.id)}`, { method: "PATCH", body: { name: String(form.get("device-name") || "").trim(), confirmActions: form.get("device-confirm") === "on" } });
    await queries.invalidateQueries({ queryKey: connectionsKey });
    onClose();
  });
  return (
    <FormDialog open={!!device} onOpenChange={(o) => !o && onClose()} title="Edit device">
      <form className="grid gap-4" onSubmit={onSubmit}>
        <TextField id="device-name" label="Name" required maxLength={40} autoComplete="off" defaultValue={device?.name || ""} />
        <CheckField id="device-confirm" label="Ask before changing calls" description="Sending, resetting, flashing and erasing wait for your Allow." defaultChecked={device?.confirmActions ?? true} />
        <FormError message={error} />
        <FormActions>
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

/** The serial monitor of an attached device: what it prints, and a line to send. */
function Monitor({ id }: { id: string }) {
  const devices = useDevices();
  const a = devices.attached(id);
  const [baud, setBaud] = useState(DEFAULT_BAUD);
  const [line, setLine] = useState("");
  const [error, setError] = useState("");
  if (!a) return null;
  const printed = a.device.recent() !== "";
  const status = a.device.status();
  const act = async (work: () => Promise<unknown>) => {
    setError("");
    try {
      await work();
    } catch (e) {
      setError((e as Error).message);
    }
  };
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Eyebrow>Monitor</Eyebrow>
        <select className="h-7 rounded-md border bg-background px-2 text-sm" value={status.baudRate ?? baud} disabled={status.open || a.device.busy} onChange={(e) => setBaud(Number(e.target.value))} aria-label="Baud rate">
          {BAUDS.map((b) => (
            <option key={b} value={b}>
              {b} baud
            </option>
          ))}
        </select>
        {status.open ? (
          <Button size="xs" variant="outline" onClick={() => act(() => a.device.close())}>
            Close port
          </Button>
        ) : (
          <Button size="xs" variant="outline" disabled={a.device.busy} onClick={() => act(() => a.device.open({ baudRate: baud }))}>
            Open port
          </Button>
        )}
        {status.open && (
          <Button size="xs" variant="ghost" onClick={() => act(() => a.device.pulse("reset"))}>
            Reset board
          </Button>
        )}
        {a.device.busy && <Pill tone="amber">Flashing</Pill>}
        {a.device.lastError && <span className="text-xs text-destructive-foreground">{a.device.lastError}</span>}
      </div>
      <div className="relative">
        <SerialTerminal device={a.device} />
        {!printed && <p className="pointer-events-none absolute inset-x-3 top-2 font-mono text-[12px] text-muted-foreground">{status.open ? "Waiting for output…" : "Open the port to see what the device prints."}</p>}
      </div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          const sent = line;
          setLine("");
          void act(() => a.device.writeText(sent));
        }}
      >
        <input className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 font-mono text-sm" value={line} onChange={(e) => setLine(e.target.value)} placeholder={status.open ? "Send a line…" : "Open the port first"} disabled={!status.open} aria-label="Line to send" />
        <Button type="submit" size="sm" variant="outline" disabled={!status.open}>
          Send
        </Button>
      </form>
      {error && <p className="text-sm text-destructive-foreground">{error}</p>}
    </div>
  );
}

const connectionOf = (c: Connection) => ({ id: c.id, name: c.name, vendorId: c.vendorId ?? undefined, productId: c.productId ?? undefined, user: c.user || undefined, secret: c.secret || undefined });

function DeviceCard({ c, onEdit, onSignIn, onRemove }: { c: Connection; onEdit: () => void; onSignIn: () => void; onRemove: () => void }) {
  const devices = useDevices();
  const a = devices.attached(c.id);
  const [busy, setBusy] = useState("");
  const [identity, setIdentity] = useState("");
  const act = async (what: string, work: () => Promise<unknown>) => {
    setBusy(what);
    try {
      await work();
    } catch (e) {
      toast((e as Error).message);
    }
    setBusy("");
  };
  return (
    <article className="grid gap-3 rounded-lg border bg-card p-4">
      <div className="flex flex-wrap items-start gap-3">
        <Cpu className="mt-0.5 size-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <h3 className="flex flex-wrap items-center gap-1.5 text-base font-semibold">
            {c.name}
            {a ? <Pill tone="accent">Attached here</Pill> : <Pill>Not attached</Pill>}
            {!c.confirmActions && <Pill tone="amber">Runs without asking</Pill>}
            {c.user && c.secret && (
              <Pill>
                <KeyRound className="mr-1 inline size-3" />
                Signs in as {c.user}
              </Pill>
            )}
          </h3>
          <p className="text-sm text-muted-foreground">{[c.label, ids(c), `${c.tools.length} tools`].filter(Boolean).join(" · ")}</p>
        </div>
        <div className="flex flex-wrap gap-1.5">
          {a ? (
            <>
              <Button size="sm" variant="outline" disabled={!!busy || a.device.busy} onClick={() => act("identify", () => identify(a).then(setIdentity))}>
                {busy === "identify" ? "Identifying…" : "Identify chip"}
              </Button>
              <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => act("detach", () => devices.detach(c.id))}>
                <Unplug /> Detach
              </Button>
            </>
          ) : (
            <Button size="sm" disabled={!!busy || !devices.supported()} onClick={() => act("attach", () => devices.attach(connectionOf(c)))}>
              <Usb /> Attach
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={onSignIn}>
            Console sign-in
          </Button>
          <Button size="sm" variant="ghost" onClick={onEdit}>
            Edit
          </Button>
          <Button size="sm" variant="ghost" onClick={onRemove}>
            Remove
          </Button>
        </div>
      </div>
      {identity && <pre className="rounded-md bg-muted px-3 py-2 font-mono text-[12px] whitespace-pre-wrap">{identity}</pre>}
      {a && <Monitor id={c.id} />}
    </article>
  );
}

/** Asks the chip what it is, through the first flasher that recognises it. */
async function identify(a: { device: { lend: <T>(work: (port: SerialPort) => Promise<T>) => Promise<T> }; vendorId?: number }) {
  const found = await a.device.lend((port) => withFlasher(a.vendorId, (f) => f.identify(port, () => {})));
  return formatIdentity(found);
}

function formatIdentity(i: { family: string; chip: string; mac?: string; flashSize?: string; features?: string[]; crystal?: string }) {
  return [`${i.family} ${i.chip}`, i.mac ? `MAC ${i.mac}` : "", i.flashSize ? `Flash ${i.flashSize}` : "", i.crystal ? `Crystal ${i.crystal}` : "", i.features?.length ? `Features: ${i.features.join(", ")}` : ""].filter(Boolean).join("\n");
}

export function DevicesPage() {
  const capabilities = useCapabilities();
  const on = !!capabilities.data?.features?.devices;
  const devices = useDevices();
  const queries = useQueryClient();
  const { data, error, isPending, refetch } = useConnections();
  const list = (data ?? []).filter(isDevice);
  const [picked, setPicked] = useState<Picked | null>(null);
  const [editing, setEditing] = useState<Connection | null>(null);
  const [signingIn, setSigningIn] = useState<Connection | null>(null);
  const confirm = useConfirm();
  usePageChrome("Compute");
  // Ports granted before are tied back to their devices when the page opens.
  useEffect(() => {
    if (list.length && devices.supported()) void devices.restore(list.map(connectionOf));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per list
  }, [data]);
  const add = async () => {
    try {
      setPicked(await devices.pick());
    } catch (e) {
      if ((e as Error).name !== "NotFoundError") toast((e as Error).message);
    }
  };
  const remove = (c: Connection) =>
    confirm.ask({
      title: `Remove ${c.name}?`,
      text: "Chats lose its tools. The board itself is not touched.",
      label: "Remove",
      tone: "danger",
      action: async () => {
        await devices.detach(c.id);
        await api.request(`/api/connections/${encodeURIComponent(c.id)}`, { method: "DELETE" });
        await queries.invalidateQueries({ queryKey: connectionsKey });
      },
    });
  if (capabilities.data && !on)
    return (
      <Page>
        <ComputeTabs />
        <ComputeOff page="devices" />
      </Page>
    );
  return (
    <Page>
      <PageHeader
        title="Devices"
        description="Boards and instruments plugged into your computer, reached through this browser over USB serial. In a chat the model can open the port, read and send, reset, identify the chip and flash firmware built in the workspace."
        actions={
          <Button onClick={add} disabled={!devices.supported()}>
            <Plus /> Add a device
          </Button>
        }
      >
        <ComputeTabs />
      </PageHeader>
      {!devices.supported() && <Note className={cn("mb-4")}>This browser has no Web Serial support. Open Chat in Chrome or Edge to add and use devices.</Note>}
      {error ? (
        <ErrorBox error={error} onRetry={() => refetch()} />
      ) : isPending ? (
        <Loading />
      ) : list.length === 0 ? (
        <EmptyState icon={Cpu} title="No devices yet." text="Plug a board in over USB and add it. Espressif chips can be identified and flashed; any serial device can be read and sent to." action={<Button onClick={add} disabled={!devices.supported()}>Add a device</Button>} />
      ) : (
        <div className="grid gap-3">
          {list.map((c) => (
            <DeviceCard key={c.id} c={c} onEdit={() => setEditing(c)} onSignIn={() => setSigningIn(c)} onRemove={() => remove(c)} />
          ))}
        </div>
      )}
      <AddDeviceDialog picked={picked} onClose={() => setPicked(null)} />
      <EditDialog device={editing} onClose={() => setEditing(null)} />
      <SignInDialog device={signingIn} onClose={() => setSigningIn(null)} />
      {confirm.element}
    </Page>
  );
}
