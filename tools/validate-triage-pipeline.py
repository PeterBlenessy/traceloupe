#!/usr/bin/env python3
"""Validate that the triage pipeline reproduces the lab result (recall 0.94 /
precision 0.95 vs the shipped batch scan's 0.30 / 0.89).

This is the PROVEN oracle: it is the harness that produced the numbers recorded
in docs/safety-scan-journey.md §6.1. It runs the three-stage pipeline —
per-message embedding census -> focused classification -> optional Guard
confirmation — over real Jigsaw-labelled threats buried in generated mundane
conversation, with prototypes built from HELD-OUT threats (no leave-one-out
inflation).

It validates the ARCHITECTURE end to end. It sends the PRODUCTION system prompt
(read from prompt.rs) and the PRODUCTION GBNF grammar (dumped by the Rust
`dump_grammars` test), so it cannot drift into reimplementing them — the
missing-grammar mistake produced false "recall 0.00" three times (journey §10.6).

--------------------------------------------------------------------------------
SETUP (once, from repo root) — see docs/validation/triage-validation-setup.md
--------------------------------------------------------------------------------
  # 1. classifier (already present if Safety Scan has downloaded it), else:
  #    it lives under the app data dir, e.g.
  #    ~/Library/Application Support/se.addable.traceloupe*/models/gemma-4-E4B-it-Q4_K_M.gguf
  # 2. embedder (318 MB):
  curl -L -o /tmp/models/embeddinggemma-300M-Q8_0.gguf \
    https://huggingface.co/ggml-org/embeddinggemma-300M-GGUF/resolve/main/embeddinggemma-300M-Q8_0.gguf
  # 3. Guard, only if validating confirm-on modes (4.6 GB):
  curl -L -o /tmp/models/llama-guard-3-8b.gguf \
    https://huggingface.co/mradermacher/Llama-Guard-3-8B-GGUF/resolve/main/Llama-Guard-3-8B.Q4_K_M.gguf
  # 4. Jigsaw threats (CC-BY-SA; eval only, do not vendor):
  curl -L -o /tmp/public-sets/jigsaw.csv \
    https://huggingface.co/datasets/tasksource/jigsaw_toxicity/resolve/main/train.csv
  # 5. production grammar:
  cargo test -p traceloupe-core --lib dump_grammars -- --ignored     # -> /tmp/grammars.json

RUN:
  TRACELOUPE_LLAMA_SERVER=src-tauri/binaries/llama-server-aarch64-apple-darwin \
  TRIAGE_GEMMA="/path/to/gemma-4-E4B-it-Q4_K_M.gguf" \
  TRIAGE_EMBED=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
  TRIAGE_GUARD=/tmp/models/llama-guard-3-8b.gguf \
  TRIAGE_JIGSAW=/tmp/public-sets/jigsaw.csv \
  TRIAGE_GRAMMARS=/tmp/grammars.json \
  python3 tools/validate-triage-pipeline.py 0.52       # threshold; sweep 0.64 0.58 0.52

PASS: end-to-end recall within ~0.05 of 0.94 at precision >= 0.90. Record the run
in docs/validation/safety-scan-validation.md and tick docs/safety-scan-journey.md §8.

No real backup data is used: threats are Jigsaw-labelled, beds are generated.
"""
import csv, json, os, random, subprocess, sys, time, urllib.request

def env(name, default=None):
    v = os.environ.get(name, default)
    if v is None:
        sys.exit(f"set {name} (see the module docstring for setup)")
    return v

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SERVER = env("TRACELOUPE_LLAMA_SERVER",
             os.path.join(ROOT, "src-tauri/binaries/llama-server-aarch64-apple-darwin"))
GEMMA = env("TRIAGE_GEMMA")
GUARD = os.environ.get("TRIAGE_GUARD", "")
EMBED = env("TRIAGE_EMBED")
JIGSAW = env("TRIAGE_JIGSAW")
GRAMMARS_PATH = env("TRIAGE_GRAMMARS", "/tmp/grammars.json")
PORT = 8621; CACHE = "/tmp/triage-validate-stages.json"; random.seed(20260811)
TH = float(sys.argv[1]) if len(sys.argv) > 1 else 0.52
src = open(os.path.join(ROOT, "crates/traceloupe-core/src/safety_scan/prompt.rs"), encoding="utf-8").read()
mk = 'pub const SYSTEM_PROMPT: &str = r#"'
SYSTEM = src[src.index(mk)+len(mk):src.index('"#', src.index(mk)+len(mk))]
G = json.load(open(GRAMMARS_PATH))
PFX = "task: classification | query: "

def cl(t):
    t=" ".join(t.split()); return t if 20<=len(t)<=300 else None
rows=list(csv.DictReader(open(JIGSAW,encoding="utf-8")))
threats=[cl(r["comment_text"]) for r in rows if r["threat"]=="1" and cl(r["comment_text"])]
clean=[cl(r["comment_text"]) for r in rows
   if all(r[k]=="0" for k in ("toxic","severe_toxic","obscene","threat","insult","identity_hate")) and cl(r["comment_text"])]
random.shuffle(threats); random.shuffle(clean)
N=80
# Each chunk: 5 messages. Positives hide one threat at index `at`; negatives are clean.
chunks=[]
for t in threats[:N]:
    at=random.randrange(5); msgs=random.sample(clean,4); msgs.insert(at,t)
    chunks.append({"msgs":msgs,"at":at,"real":True})
for _ in range(N):
    chunks.append({"msgs":random.sample(clean,5),"at":None,"real":False})

def serve(m,extra=(),ctx="8192"):
    if not os.path.exists(m): sys.exit(f"MISSING: {m}")
    p=subprocess.Popen([SERVER,"--model",m,"--host","127.0.0.1","--port",str(PORT),
      "-ngl","-1","--ctx-size",ctx,*extra],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    for _ in range(500):
        try:
            if b"ok" in urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health",timeout=2).read(): return p
        except Exception: time.sleep(1)
    p.terminate(); sys.exit("unhealthy")
def post(path,body,to=600):
    r=urllib.request.Request(f"http://127.0.0.1:{PORT}{path}",data=json.dumps(body).encode(),
      headers={"Content-Type":"application/json"})
    return json.load(urllib.request.urlopen(r,timeout=to))
stages=json.load(open(CACHE)) if os.path.exists(CACHE) else {}

# ---- baseline: shipped BATCH scan (one verdict call per whole chunk) ----
if "batch" not in stages:
    p=serve(GEMMA)
    try:
        res=[]
        for c in chunks:
            lines=[f"[{i}] {'me' if i%2 else 'them'}: {t}" for i,t in enumerate(c["msgs"])]
            o=post("/v1/chat/completions",{"temperature":0,"max_tokens":900,
              "grammar":G[str(len(c["msgs"]))],
              "messages":[{"role":"system","content":SYSTEM},
                          {"role":"user","content":"Conversation: p\n"+"\n".join(lines)}]})
            txt=o["choices"][0]["message"]["content"]
            try: j=json.loads(txt[txt.index("{"):txt.rindex("}")+1])
            except Exception: j={"verdicts":[]}
            hit=any(v.get("category")=="threat-violence" and isinstance(v.get("severity"),int) and v["severity"]>=2
                    for v in j.get("verdicts",[]))
            res.append(hit)
        stages["batch"]=res
    finally: p.terminate(); p.wait()
    json.dump(stages,open(CACHE,"w"))

# ---- stage 1: census, per message ----
if "census" not in stages:
    p=serve(EMBED,("--embedding","--pooling","mean"),"2048")
    try:
        def emb(t):
            d=post("/embedding",{"content":PFX+t})
            return d[0]["embedding"][0] if isinstance(d,list) else d["embedding"]
        proto=[emb(t) for t in threats[N:N+30]]  # prototypes from HELD-OUT threats
        cent=[sum(col)/len(col) for col in zip(*proto)]
        def cos(a,b):
            d=sum(x*y for x,y in zip(a,b)); na=sum(x*x for x in a)**.5; nb=sum(x*x for x in b)**.5
            return d/(na*nb) if na and nb else 0
        # keep a chunk if ANY message scores >= TH; record which message(s)
        survive=[]
        for c in chunks:
            scored=[cos(emb(m),cent) for m in c["msgs"]]
            keep=[i for i,s in enumerate(scored) if s>=TH]
            survive.append(keep)
        stages["census"]=survive
    finally: p.terminate(); p.wait()
    json.dump(stages,open(CACHE,"w"))

# ---- stage 2: focused classify each surviving message ----
if "focused" not in stages:
    p=serve(GEMMA)
    try:
        found=[]  # list of (chunk_idx, msg_idx) flagged as threat>=2
        for ci,(c,keep) in enumerate(zip(chunks,stages["census"])):
            for at in keep:
                lines=[f"[{i}] {'me' if i%2 else 'them'}: {t}" for i,t in enumerate(c["msgs"])]
                user=("Conversation: p\n"+"\n".join(lines)+
                      f"\n\nJudge ONLY item [{at}]. The other items are context. "
                      f"Output a verdict for item [{at}] only, or none.")
                o=post("/v1/chat/completions",{"temperature":0,"max_tokens":600,
                  "grammar":G[str(len(c["msgs"]))],
                  "messages":[{"role":"system","content":SYSTEM},{"role":"user","content":user}]})
                txt=o["choices"][0]["message"]["content"]
                try: j=json.loads(txt[txt.index("{"):txt.rindex("}")+1])
                except Exception: j={"verdicts":[]}
                # clamp to the focused item
                if any(v.get("index")==at and isinstance(v.get("severity"),int) and v["severity"]>=2
                       for v in j.get("verdicts",[])):
                    found.append([ci,at])
        stages["focused"]=found
    finally: p.terminate(); p.wait()
    json.dump(stages,open(CACHE,"w"))

# ---- stage 3: guard confirm each finding ----
CATS=[("Threats and Violence","Should not\n - Threaten violence or harm against a person.\nCan\n - Discuss violence in fiction, news or sport.\n - Quote a threat made TO the speaker.")]
block="\n".join(f"S{i+1}: {n}.\n{d}" for i,(n,d) in enumerate(CATS))
if "guard" not in stages:
    p=serve(GUARD,(),"16384")
    try:
        kept=[]
        for ci,at in stages["focused"]:
            msgs=chunks[ci]["msgs"]
            turns=[f"{'User' if i%2==0 else 'Agent'}: {t}" for i,t in enumerate(msgs)]
            turns.append(f"Agent: {msgs[at]}")
            prompt=("<|begin_of_text|><|start_header_id|>user<|end_header_id|>\n\nTask: Check if there is "
              "unsafe content in 'Agent' messages in conversations according our safety policy with the "
              "below categories.\n\n<BEGIN UNSAFE CONTENT CATEGORIES>\n"+block+"\n<END UNSAFE CONTENT "
              "CATEGORIES>\n\n<BEGIN CONVERSATION>\n\n"+"\n\n".join(turns)+"\n\n<END CONVERSATION>\n\n"
              "Provide your safety assessment for ONLY THE LAST Agent message:\n - First line 'safe' or "
              "'unsafe'.\n - If unsafe, a second line lists the categories.<|eot_id|>"
              "<|start_header_id|>assistant<|end_header_id|>\n\n")
            said=post("/completion",{"prompt":prompt,"temperature":0,"n_predict":24})["content"].strip().lower()
            if said.startswith("unsafe"): kept.append([ci,at])
        stages["guard"]=kept
    finally: p.terminate(); p.wait()
    json.dump(stages,open(CACHE,"w"))

# ---- report ----
def recall_prec(findings):
    real=sum(1 for ci,_ in findings if chunks[ci]["real"])
    tot_real=sum(1 for c in chunks if c["real"])
    return real/tot_real, (real/len(findings) if findings else 1.0)
b=stages["batch"]; b_real=sum(1 for i,c in enumerate(chunks) if c["real"] and b[i]); b_fp=sum(1 for i,c in enumerate(chunks) if not c["real"] and b[i])
tot_real=sum(1 for c in chunks if c["real"])
census_kept=sum(1 for k in stages["census"] if k)
census_real_kept=sum(1 for i,(c,k) in enumerate(zip(chunks,stages["census"])) if c["real"] and k)
f_r,f_p=recall_prec(stages["focused"]); g_r,g_p=recall_prec(stages["guard"])
print(f"\n=== END TO END, threshold {TH}, {len(chunks)} chunks ({tot_real} with a real threat) ===\n")
print(f"BASELINE  shipped batch scan   recall {b_real/tot_real:.2f}  precision {b_real/max(b_real+b_fp,1):.2f}")
print(f"\nstage 1  census keeps {census_kept}/{len(chunks)} chunks, {census_real_kept}/{tot_real} real  "
      f"(census recall {census_real_kept/tot_real:.2f} — a miss here is permanent)")
print(f"stage 2  focused: {len(stages['focused'])} findings  recall {f_r:.2f}  precision {f_p:.2f}")
print(f"stage 3  +guard : {len(stages['guard'])} findings  recall {g_r:.2f}  precision {g_p:.2f}")
print(f"\nPIPELINE end to end   recall {g_r:.2f}   precision {g_p:.2f}   vs batch recall {b_real/tot_real:.2f}")
