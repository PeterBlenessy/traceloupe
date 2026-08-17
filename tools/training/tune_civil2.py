"""Recalibrate the civil2 heads: finer threshold grid, larger calibration
sample, and report the full catch-vs-false-alarm curve per head so the
deployment point is a choice, not an accident."""
import random, warnings, time
warnings.filterwarnings("ignore")
import torch, numpy as np
from datasets import load_from_disk
from transformers import AutoTokenizer, AutoModelForSequenceClassification
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
random.seed(1)
DIMS=['toxicity','threat','insult','identity_attack','sexual_explicit']
ds=load_from_disk(SP+'/datasets/civil_comments')
def collect(split, cap_scan=None, cap_neg=40000):
    rows=[]; neg=[]
    for i,r in enumerate(ds[split]):
        if cap_scan and i>=cap_scan: break
        y=[1.0 if r[d]>=0.5 else 0.0 for d in DIMS]
        (rows if max(y)>0 else neg).append((r['text'],y))
    random.shuffle(neg)
    return rows+neg[:cap_neg]
cal=collect('validation')            # full validation split
test=collect('test')                 # full test split
print(f"cal {len(cal)} test {len(test)}", flush=True)
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
model=AutoModelForSequenceClassification.from_pretrained("answerdotai/ModernBERT-base",
    num_labels=len(DIMS), problem_type="multi_label_classification")
saved=torch.load(SP+'/modernbert_civil2.pt')
model.load_state_dict(saved['state'])
dev="mps" if torch.backends.mps.is_available() else "cpu"; model.to(dev); model.eval()
def scores(rows,label):
    rows=sorted(rows,key=lambda r:len(r[0]))
    out=[]; ys=[]
    t0=time.time()
    with torch.no_grad():
        for j in range(0,len(rows),64):
            xs=[t for t,_ in rows[j:j+64]]; ys+= [y for _,y in rows[j:j+64]]
            enc=tok(xs,return_tensors='pt',padding=True,truncation=True,max_length=192).to(dev)
            out.append(torch.sigmoid(model(**enc).logits).cpu())
            if j % 12800 == 0: print(f"  {label} {j}/{len(rows)} {time.time()-t0:.0f}s", flush=True)
    return torch.cat(out), torch.tensor(ys)
pc,yc=scores(cal,"cal"); pt,yt=scores(test,"test")
grid=sorted(set([x/1000 for x in range(10,1000,10)]+[x/10000 for x in range(9900,10000,10)]))
print("\nper-head curve (calibrated on full val, reported on full test):", flush=True)
for j,d in enumerate(DIMS):
    print(f"  {d}:", flush=True)
    for target in [0.005, 0.01, 0.02]:
        best=(None,-1)
        for t in grid:
            pred=pc[:,j]>=t
            fa=int((pred&(yc[:,j]==0)).sum()); nn=int((yc[:,j]==0).sum())
            if fa/nn<=target:
                c=int((pred&(yc[:,j]>0)).sum())
                if c>best[1]: best=(t,c)
        if best[0] is None: continue
        t=best[0]
        pred=pt[:,j]>=t
        tp=int((pred&(yt[:,j]>0)).sum()); fn=int(((~pred)&(yt[:,j]>0)).sum())
        fp=int((pred&(yt[:,j]==0)).sum()); tn=int(((~pred)&(yt[:,j]==0)).sum())
        print(f"    target {target:.1%}: th={t:.3f} -> caught {tp}/{tp+fn} ({tp/max(tp+fn,1):.0%}), test false alarms {fp/max(fp+tn,1):.2%}", flush=True)
