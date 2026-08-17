import random, warnings, json, time
warnings.filterwarnings("ignore")
import numpy as np
import onnxruntime as ort
from datasets import load_from_disk
from transformers import AutoTokenizer
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
random.seed(1)
DIMS=['toxicity','threat','insult','identity_attack','sexual_explicit']
ds=load_from_disk(SP+'/datasets/civil_comments')
rows=[]; neg=[]
for r in ds['validation']:
    y=[1.0 if r[d]>=0.5 else 0.0 for d in DIMS]
    (rows if max(y)>0 else neg).append((r['text'],y))
random.shuffle(neg)
cal=sorted(rows+neg[:40000],key=lambda x:len(x[0]))
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
sess=ort.InferenceSession(SP+'/civil2_onnx/model_int8.onnx', providers=['CPUExecutionProvider'])
out=[]; ys=[]; t0=time.time()
for j in range(0,len(cal),64):
    xs=[t for t,_ in cal[j:j+64]]; ys+=[y for _,y in cal[j:j+64]]
    enc=tok(xs,return_tensors='np',padding=True,truncation=True,max_length=192)
    out.append(1/(1+np.exp(-sess.run(None,dict(enc))[0])))
    if j%12800==0: print(f"{j}/{len(cal)} {time.time()-t0:.0f}s",flush=True)
p=np.concatenate(out); y=np.array(ys)
grid=sorted(set([x/1000 for x in range(10,1000,5)]+[x/10000 for x in range(9900,10000,5)]))
th={}
for j,d in enumerate(DIMS):
    th[d]={}
    for target in [0.01,0.02]:
        best=(None,-1)
        for t in grid:
            pred=p[:,j]>=t
            fa=int((pred&(y[:,j]==0)).sum()); nn=int((y[:,j]==0).sum())
            if fa/nn<=target:
                c=int((pred&(y[:,j]>0)).sum())
                if c>best[1]: best=(t,c)
        th[d][f"{target:.0%}"]=best[0]
json.dump(th, open(SP+'/civil2_int8_thresholds.json','w'), indent=1)
print(json.dumps(th),flush=True)
