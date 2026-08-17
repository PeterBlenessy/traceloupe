"""Self-harm risk on the C-SSRS expert-graded set (CC BY 4.0): the first
trusted number for this category. Task: AT-RISK (Ideation/Behavior/Attempt)
vs NOT (Supportive/Indicator) — the clinical line C-SSRS itself draws.
500 users, so 5-fold cross-validation, not a single split; report the pooled
confusion. Published reference: Gaur et al. report ~0.7-0.8 range ordinal
metrics on this data; our binary task should land at or above."""
import csv, random, warnings, time, ast
warnings.filterwarnings("ignore")
import torch
from transformers import AutoTokenizer, AutoModelForSequenceClassification
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
torch.manual_seed(0); random.seed(0)
RISK={'Ideation','Behavior','Attempt'}
rows=[]
for r in csv.DictReader(open('/Users/peter/.traceloupe-dev/datasets/cssrs/500_users_posts_labels.csv')):
    try: posts=ast.literal_eval(r['Post'])
    except Exception: posts=[r['Post']]
    text="\n".join(str(p) for p in posts)[:6000]
    rows.append((text, 1 if r['Label'] in RISK else 0))
random.shuffle(rows)
print(f"{len(rows)} users, {sum(y for _,y in rows)} at-risk", flush=True)
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
dev="mps" if torch.backends.mps.is_available() else "cpu"
def collate(b):
    xs,ys=zip(*b)
    enc=tok(list(xs),return_tensors='pt',padding=True,truncation=True,max_length=512)
    return enc, torch.tensor(ys)
K=5
tp=fp=fn=tn=0
for fold in range(K):
    test=[r for i,r in enumerate(rows) if i%K==fold]
    train=[r for i,r in enumerate(rows) if i%K!=fold]
    model=AutoModelForSequenceClassification.from_pretrained("answerdotai/ModernBERT-base", num_labels=2).to(dev)
    opt=torch.optim.AdamW(model.parameters(), lr=1e-5)
    batches=[train[i:i+8] for i in range(0,len(train),8)]
    model.train(); t0=time.time()
    for ep in range(4):
        random.shuffle(batches)
        tl=0.0
        for b in batches:
            enc,y=collate(b)
            enc={k:v.to(dev) for k,v in enc.items()}
            loss=torch.nn.functional.cross_entropy(model(**enc).logits, y.to(dev))
            loss.backward(); opt.step(); opt.zero_grad(); tl+=loss.item()
        print(f"  fold {fold} ep{ep} loss {tl/len(batches):.4f}", flush=True)
    model.eval()
    with torch.no_grad():
        for j in range(0,len(test),8):
            enc,y=collate(test[j:j+8])
            enc={k:v.to(dev) for k,v in enc.items()}
            p=model(**enc).logits.argmax(-1).cpu()
            for pi,yi in zip(p,y):
                if yi==1: tp+=pi==1; fn+=pi==0
                else: tn+=pi==0; fp+=pi==1
    print(f"fold {fold}: cumulative caught {tp}/{tp+fn}, clean kept {tn}/{tn+fp} ({time.time()-t0:.0f}s)", flush=True)
    del model
prec=tp/max(tp+fp,1); rec=tp/max(tp+fn,1)
print(f"CSSRS 5-fold: caught {tp}/{tp+fn} ({rec:.0%}), false alarms {fp}/{fp+tn} ({fp/max(fp+tn,1):.0%}), precision {prec:.0%}", flush=True)
