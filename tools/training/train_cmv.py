"""First real-data run: ModernBERT-base on CGA conversation-level derailment.
Best-epoch checkpointing (val loss), report the published test split, then the
226-case behavioural checklist. Sender roles rendered as A:/B: tokens."""
import json, glob, os, random, warnings
warnings.filterwarnings("ignore")
import torch
from torch.utils.data import DataLoader, Dataset
from transformers import AutoTokenizer, AutoModelForSequenceClassification
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
ROOT=SP+'/datasets/conversations-gone-awry-cmv-corpus'
REPO='/Users/peter/Development/iphone-backup-analyzer'
torch.manual_seed(0); random.seed(0)

convos=json.load(open(ROOT+'/conversations.json'))
utts={}
for line in open(ROOT+'/utterances.jsonl'):
    u=json.loads(line)
    utts.setdefault(u['conversation_id'],[]).append(u)
def render(cid):
    us=sorted(utts[cid], key=lambda u:(u.get('timestamp') or 0))
    speakers={}
    out=[]
    for u in us:
        s=speakers.setdefault(u['speaker'], chr(ord('A')+len(speakers)%26))
        t=(u.get('text') or '').strip()
        if t: out.append(f"{s}: {t}")
    return "\n".join(out)
splits={'train':[], 'val':[], 'test':[]}
for cid,meta in convos.items():
    splits[meta['meta']['split']].append((render(cid), 1 if meta['meta']['has_removed_comment'] else 0))
print({k:len(v) for k,v in splits.items()}, flush=True)

tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
model=AutoModelForSequenceClassification.from_pretrained("answerdotai/ModernBERT-base", num_labels=2)
dev="mps" if torch.backends.mps.is_available() else "cpu"; model.to(dev)
print("device:", dev, flush=True)
class DS(Dataset):
    def __init__(s,rows): s.rows=rows
    def __len__(s): return len(s.rows)
    def __getitem__(s,i): return s.rows[i]
def collate(b):
    xs,ys=zip(*b)
    enc=tok(list(xs), return_tensors='pt', padding=True, truncation=True, max_length=512)
    return enc, torch.tensor(ys)
dl=DataLoader(DS(splits['train']),batch_size=8,shuffle=True,collate_fn=collate)
vdl=DataLoader(DS(splits['val']),batch_size=8,collate_fn=collate)
opt=torch.optim.AdamW(model.parameters(), lr=2e-5)
best=(1e9,None)
for ep in range(3):
    model.train(); tl=0
    import time; t0=time.time()
    for i,(enc,y) in enumerate(dl):
        enc={k:v.to(dev) for k,v in enc.items()}; y=y.to(dev)
        out=model(**enc, labels=y); out.loss.backward(); opt.step(); opt.zero_grad(); tl+=out.loss.item()
        if i%50==0: print(f"  ep{ep} step {i}/{len(dl)} {(time.time()-t0)/(i+1):.2f}s/step", flush=True)
    model.eval(); vl=0; vc=0; vn=0
    with torch.no_grad():
        for enc,y in vdl:
            enc={k:v.to(dev) for k,v in enc.items()}; y=y.to(dev)
            out=model(**enc, labels=y); vl+=out.loss.item()
            vc+=(out.logits.argmax(-1)==y).sum().item(); vn+=len(y)
    print(f"epoch {ep}: train_loss {tl/len(dl):.4f} val_loss {vl/len(vdl):.4f} val_acc {vc/vn:.3f}", flush=True)
    if vl<best[0]:
        best=(vl,{k:v.cpu().clone() for k,v in model.state_dict().items()})
model.load_state_dict(best[1]); model.eval()
def acc(rows):
    c=0; tp=fp=fn=tn=0
    with torch.no_grad():
        for i in range(0,len(rows),8):
            xs,ys=zip(*rows[i:i+8])
            enc=tok(list(xs),return_tensors='pt',padding=True,truncation=True,max_length=512).to(dev)
            p=model(**enc).logits.argmax(-1).cpu()
            for pi,yi in zip(p,ys):
                c+= pi==yi
                if yi==1: tp+=pi==1; fn+=pi==0
                else: tn+=pi==0; fp+=pi==1
    return c/len(rows), tp, fn, fp, tn
a,tp,fn,fp,tn=acc(splits['test'])
print(f"CGA TEST: acc {a:.3f}  (attacks caught {tp}/{tp+fn}, clean kept clean {tn}/{tn+fp})", flush=True)
# behavioural checklist
cases=[]
for f in sorted(glob.glob(REPO+'/crates/traceloupe-core/fixtures/safety-scan/eval/*.jsonl')):
    for l in open(f):
        d=json.loads(l)
        txt="\n".join(f"{'A' if m['sender']=='them' else 'B'}: {m['text']}" for m in d['messages'])
        cases.append((txt, 1 if d['kind']=='positive' else 0))
a,tp,fn,fp,tn=acc(cases)
print(f"CHECKLIST: acc {a:.3f}  (harmful caught {tp}/{tp+fn}, ordinary kept clean {tn}/{tn+fp})", flush=True)
torch.save(best[1], SP+'/modernbert_cmv.pt')
print("saved", flush=True)
