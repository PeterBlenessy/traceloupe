"""PAN12 predatory-conversation identification, Fauzi & Bours protocol:
binary conversation classification, report precision/recall/F1/F0.5 on the
official test corpus. Published reference to be in range of: F0.5 ~0.93."""
import xml.etree.ElementTree as ET, os, random, warnings, time, json, glob
warnings.filterwarnings("ignore")
import torch
from transformers import AutoTokenizer, AutoModelForSequenceClassification
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
ROOT=os.path.expanduser('~/.traceloupe-dev/datasets/pan12')
TR=ROOT+'/pan12-sexual-predator-identification-training-corpus-2012-05-01'
TE=ROOT+'/pan12-sexual-predator-identification-test-corpus-2012-05-21'
torch.manual_seed(0); random.seed(0)
def load(xml, preds_file):
    preds=set(open(preds_file).read().split())
    out=[]
    for c in ET.parse(xml).getroot().findall('conversation'):
        speakers={}; lines=[]
        hit=False
        for m in c.findall('message'):
            a=m.find('author').text; t=(m.find('text').text or '').strip()
            if a in preds: hit=True
            if not t: continue
            s=speakers.setdefault(a, chr(ord('A')+len(speakers)%26))
            lines.append(f"{s}: {t}")
        if len(lines)>=3 and len(speakers)>=2:
            out.append(("\n".join(lines), 1 if hit else 0))
    return out
train_all=load(TR+'/pan12-sexual-predator-identification-training-corpus-2012-05-01.xml',
               TR+'/pan12-sexual-predator-identification-training-corpus-predators-2012-05-01.txt')
random.shuffle(train_all)
val=train_all[:3000]; train=train_all[3000:]
print(f"train {len(train)} ({sum(y for _,y in train)} predatory) val {len(val)} ({sum(y for _,y in val)})", flush=True)
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
model=AutoModelForSequenceClassification.from_pretrained("answerdotai/ModernBERT-base", num_labels=2)
dev="mps" if torch.backends.mps.is_available() else "cpu"; model.to(dev)
print("device:",dev,flush=True)
w=torch.tensor([1.0, 12.0]).to(dev)   # ~3% positive
def collate(b):
    xs,ys=zip(*b)
    enc=tok(list(xs),return_tensors='pt',padding=True,truncation=True,max_length=256)
    return enc, torch.tensor(ys)
train=sorted(train, key=lambda r: len(r[0]))
batches=[train[i:i+16] for i in range(0,len(train),16)]
random.shuffle(batches)
opt=torch.optim.AdamW(model.parameters(), lr=2e-5)
best=(1e9,None)
for ep in range(2):
    model.train(); t0=time.time()
    for i,b in enumerate(batches):
        enc,y=collate(b)
        enc={k:v.to(dev) for k,v in enc.items()}; y=y.to(dev)
        loss=torch.nn.functional.cross_entropy(model(**enc).logits, y, weight=w)
        loss.backward(); opt.step(); opt.zero_grad()
        if i%200==0: print(f"  ep{ep} step {i}/{len(batches)} {(time.time()-t0)/(i+1):.2f}s/step",flush=True)
    model.eval(); vl=0; n=0
    with torch.no_grad():
        for j in range(0,len(val),16):
            enc,y=collate(val[j:j+16])
            enc={k:v.to(dev) for k,v in enc.items()}
            vl+=torch.nn.functional.cross_entropy(model(**enc).logits, y.to(dev), weight=w).item(); n+=1
    print(f"epoch {ep}: val_loss {vl/n:.4f}",flush=True)
    if vl<best[0]: best=(vl,{k:v.cpu().clone() for k,v in model.state_dict().items()})
    random.shuffle(batches)
model.load_state_dict(best[1]); model.eval()
torch.save(best[1], SP+'/modernbert_pan12.pt'); print("weights saved",flush=True)
test=load(TE+'/pan12-sexual-predator-identification-test-corpus-2012-05-17.xml',
          TE+'/pan12-sexual-predator-identification-groundtruth-problem1.txt')
test=sorted(test,key=lambda r: len(r[0]))
print(f"test {len(test)} ({sum(y for _,y in test)} predatory)",flush=True)
tp=fp=fn=tn=0; t0=time.time()
with torch.no_grad():
    for j in range(0,len(test),32):
        enc,y=collate(test[j:j+32])
        enc={k:v.to(dev) for k,v in enc.items()}
        p=model(**enc).logits.argmax(-1).cpu()
        for pi,yi in zip(p,y):
            if yi==1: tp+=pi==1; fn+=pi==0
            else: fp+=pi==1; tn+=pi==0
        if j%6400==0: print(f"  test {j}/{len(test)} {(time.time()-t0):.0f}s",flush=True)
prec=tp/max(tp+fp,1); rec=tp/max(tp+fn,1)
f1=2*prec*rec/max(prec+rec,1e-9); f05=(1.25*prec*rec)/max(0.25*prec+rec,1e-9)
print(f"PAN12 TEST: precision {prec:.3f} recall {rec:.3f} F1 {f1:.3f} F0.5 {f05:.3f}",flush=True)
print(f"  (caught {tp}/{tp+fn} predatory, false alarms {fp}/{fp+tn})",flush=True)
