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
            Run the deep read on a model you choose — Ollama, LM Studio, vLLM or
            a hosted API — instead of the built-in one. Useful if you already run
            something larger and want its judgement, or a faster machine to run
            it on.
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

          <div className="rounded-md border p-3 text-xs">
            <p className="font-medium">What your model would receive</p>
            <p className="mt-1 text-muted-foreground">
              The messages the pre-scan picks out — usually a small share of a
              backup — each with the two messages either side for context, the
              sender, the time and the conversation name. Photos and attachments
              are never sent, and neither is anything the pre-scan didn't pick
              out. The pre-scan itself always runs on this Mac.
            </p>
            {remote ? (
              <p className="mt-2">
                Because this address isn't on this Mac, those messages leave the
                device. They're also the ones most likely to be sensitive, since
                that's what the pre-scan selects for — so it's worth being
                comfortable with whoever runs that server, and with their terms
                on keeping or training on what they receive. You'll be asked to
                confirm each time you start a scan, and you can switch back to
                the built-in model whenever you like.
              </p>
            ) : (
              <p className="mt-2 text-muted-foreground">
                This address is on this Mac, so nothing leaves the device.
              </p>
            )}
          </div>

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
