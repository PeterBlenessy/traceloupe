import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Server, XCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { client } from "@/lib/ipc";
import { usePersistedState } from "@/lib/use-persisted-state";

/**
 * Bring your own model: point the deep scan at an endpoint you run.
 *
 * The address and model name persist; the API key does NOT — it goes straight
 * to the system keychain and is never read back into this view. The
 * acknowledgement is deliberately not persisted either: the backend requires it
 * per scan, because permission to send one device's messages somewhere should
 * not outlive the scan it was given for.
 */
export function EndpointSettings() {
  const qc = useQueryClient();
  const [url, setUrl] = usePersistedState("safety-scan:endpoint-url", "");
  const [model, setModel] = usePersistedState("safety-scan:endpoint-model", "");
  const [enabled, setEnabled] = usePersistedState(
    "safety-scan:endpoint-enabled",
    false,
  );
  const [key, setKey] = useState("");

  const hasKey = useQuery({
    queryKey: ["endpoint", "hasKey"],
    queryFn: () => client.hasEndpointApiKey(),
  });
  const saveKey = useMutation({
    mutationFn: (k: string) => client.setEndpointApiKey(k),
    onSuccess: () => {
      setKey("");
      void qc.invalidateQueries({ queryKey: ["endpoint", "hasKey"] });
    },
  });
  const test = useMutation({
    mutationFn: () =>
      client.testEndpoint({ url, model, acknowledged: true }),
  });

  const remote = /^https?:\/\//.test(url) && !/^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])/.test(url);

  return (
    <div className="rounded-lg border p-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h3 className="flex items-center gap-2 text-sm font-medium">
            <Server className="size-4" /> Use your own model
          </h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Send the deep scan to a model you run — Ollama, LM Studio, vLLM, or
            a hosted API — instead of the built-in one. The pre-scan always stays
            on this device, so at most the small share of messages the scan picks
            out is ever sent.
          </p>
        </div>
        <Tooltip>
          <TooltipTrigger asChild>
            <div>
              <Switch
                checked={enabled}
                onCheckedChange={setEnabled}
                aria-label="Use your own model for the deep scan"
              />
            </div>
          </TooltipTrigger>
          <TooltipContent>
            {enabled
              ? "Turn off to use the built-in model"
              : "Turn on to send the deep scan to your own model server"}
          </TooltipContent>
        </Tooltip>
      </div>

      {enabled && (
        <div className="mt-4 space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="endpoint-url" className="text-xs">
              Server address
            </Label>
            <Input
              id="endpoint-url"
              placeholder="http://127.0.0.1:11434/v1"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="endpoint-model" className="text-xs">
              Model name
            </Label>
            <Input
              id="endpoint-model"
              placeholder="llama3.1:70b"
              value={model}
              onChange={(e) => setModel(e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="endpoint-key" className="text-xs">
              API key {hasKey.data ? "(one is saved)" : "(if your server needs one)"}
            </Label>
            <div className="flex gap-2">
              <Input
                id="endpoint-key"
                type="password"
                placeholder={hasKey.data ? "••••••••" : "not set"}
                value={key}
                onChange={(e) => setKey(e.target.value)}
              />
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="outline"
                    disabled={saveKey.isPending}
                    onClick={() => saveKey.mutate(key)}
                  >
                    Save
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  Store the key in your Mac's keychain — it is never kept in the
                  app's own storage. Save an empty box to remove it.
                </TooltipContent>
              </Tooltip>
            </div>
          </div>

          {remote && (
            <p className="rounded-md border border-status-warn-border bg-status-warn-bg p-2 text-xs text-status-warn-text">
              This server is not on this machine. The messages the scan reads in
              depth will be sent to it, and what happens to them there is
              governed by whoever runs it. You will be asked to confirm each time
              you start a scan.
            </p>
          )}

          <div className="flex items-center gap-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={test.isPending || !url || !model}
                  onClick={() => test.mutate()}
                >
                  {test.isPending ? "Checking…" : "Check connection"}
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                Ask the server whether it answers — no message content is sent
              </TooltipContent>
            </Tooltip>
            {test.isSuccess && (
              <span className="flex items-center gap-1.5 text-xs">
                <CheckCircle2 className="size-4 shrink-0 text-status-ok-text" />
                {test.data}
              </span>
            )}
            {test.isError && (
              <span className="flex items-center gap-1.5 text-xs">
                <XCircle className="size-4 shrink-0 text-destructive" />
                {String(test.error).replace(/^Error:\s*/, "")}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
