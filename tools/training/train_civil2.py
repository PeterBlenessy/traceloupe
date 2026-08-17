"""Civil heads, second iteration (#518): keep EVERY example of the rare
dimensions (threat / identity_attack / sexual_explicit), cap the abundant
ones, and tune per-head thresholds on validation for a 1% false-alarm target
instead of assuming 0.5 means anything."""
import json, random, warnings, time
warnings.filterwarnings("ignore")
import torch
from datasets import load_from_disk
from transformers import AutoTokenizer, AutoModelForSequenceClassification
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
torch.manual_seed(0); random.seed(0)
DIMS=['toxicity','threat','insult','identity_attack','sexual_explicit']
RARE={'threat','identity_attack','sexual_explicit'}
ds=load_from_disk(SP+'/datasets/civil_comments')
def label(r): return [1.0 if r[d]>=0.5 else 0.0 for d in DIMS]
def collect(split, cap_common=15000, cap_neg=25000, cap_scan=None):
    rare,common,neg=[],[],[]
    for i,r in enumerate(ds[split]):
        if cap_scan and i>=cap_scan: break
        y=label(r)
        if any(y[DIMS.index(d)]>0 for d in RARE): rare.append((r['text'],y))
        elif max(y)>0: common.append((r['text'],y))
        else: neg.append((r['text'],y))
    random.shuffle(common); random.shuffle(neg)
    out=rare+common[:cap_common]+neg[:cap_neg]; random.shuffle(out)
    print(f"{split}: rare {len(rare)} common {len(common[:cap_common])} neg {len(neg[:cap_neg])}", flush=True)
    return out
train=collect('train')
val=collect('validation', 3000, 5000)
test=collect('test', 4000, 6000)
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
model=AutoModelForSequenceClassification.from_pretrained("answerdotai/ModernBERT-base",
    num_labels=len(DIMS), problem_type="multi_label_classification")
dev="mps" if torch.backends.mps.is_available() else "cpu"; model.to(dev)
print("device:",dev,flush=True)
# pos_weight per head from the actual mix, so rare heads learn
import numpy as np
Y=np.array([y for _,y in train])
pw=torch.tensor(((len(Y)-Y.sum(0))/np.maximum(Y.sum(0),1)).clip(1,20), dtype=torch.float32).to(dev)
print("pos_weight:", [f"{d}:{w:.1f}" for d,w in zip(DIMS,pw.tolist())], flush=True)
def collate(b):
    xs,ys=zip(*b)
    enc=tok(list(xs),return_tensors='pt',padding=True,truncation=True,max_length=192)
    return enc, torch.tensor(ys)
train=sorted(train,key=lambda r:len(r[0]))
batches=[train[i:i+16] for i in range(0,len(train),16)]
random.shuffle(batches)
opt=torch.optim.AdamW(model.parameters(),lr=2e-5)
lossf=torch.nn.BCEWithLogitsLoss(pos_weight=pw)
best=(1e9,None)
for ep in range(2):
    model.train(); t0=time.time()
    for i,b in enumerate(batches):
        enc,y=collate(b)
        enc={k:v.to(dev) for k,v in enc.items()}
        loss=lossf(model(**enc).logits, y.to(dev))
        loss.backward(); opt.step(); opt.zero_grad()
        if i%200==0: print(f"  ep{ep} step {i}/{len(batches)} {(time.time()-t0)/(i+1):.2f}s/step",flush=True)
    model.eval(); vl=0;n=0
    with torch.no_grad():
        for j in range(0,len(val),32):
            enc,y=collate(val[j:j+32])
            enc={k:v.to(dev) for k,v in enc.items()}
            vl+=lossf(model(**enc).logits,y.to(dev)).item(); n+=1
    print(f"epoch {ep}: val_loss {vl/n:.4f}",flush=True)
    if vl<best[0]: best=(vl,{k:v.cpu().clone() for k,v in model.state_dict().items()})
    random.shuffle(batches)
model.load_state_dict(best[1]); model.eval()
def scores(rows):
    out=[]
    with torch.no_grad():
        for j in range(0,len(rows),32):
            enc,_=collate(rows[j:j+32])
            enc={k:v.to(dev) for k,v in enc.items()}
            out.append(torch.sigmoid(model(**enc).logits).cpu())
    return torch.cat(out), torch.tensor([y for _,y in rows])
# per-head threshold: highest catch subject to <=1% false alarms on val
pv,yv=scores(val)
TH={}
for j,d in enumerate(DIMS):
    best_t=0.5; best_c=-1
    for t in [x/100 for x in range(5,96,5)]:
        pred=pv[:,j]>=t
        fa=int((pred&(yv[:,j]==0)).sum()); n_neg=int((yv[:,j]==0).sum())
        if fa/n_neg<=0.01:
            c=int((pred&(yv[:,j]>0)).sum())
            if c>best_c: best_c,best_t=c,t
    TH[d]=best_t
print("thresholds:", TH, flush=True)
pt,yt=scores(test)
for j,d in enumerate(DIMS):
    pred=pt[:,j]>=TH[d]
    tp=int((pred&(yt[:,j]>0)).sum()); fn=int(((~pred)&(yt[:,j]>0)).sum())
    fp=int((pred&(yt[:,j]==0)).sum()); tn=int(((~pred)&(yt[:,j]==0)).sum())
    print(f"CIVIL2 {d:16} caught {tp}/{tp+fn} ({tp/max(tp+fn,1):.0%})  false alarms {fp}/{fp+tn} ({fp/max(fp+tn,1):.2%})",flush=True)
torch.save({'state':best[1],'thresholds':TH}, SP+'/modernbert_civil2.pt'); print("saved",flush=True)
