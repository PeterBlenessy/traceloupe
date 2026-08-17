"""Does quantisation survive calibration? Score full val+test with fp32, int8,
int8-per-channel; calibrate thresholds per variant on ITS OWN val scores; report
test catch at the 1% and 2% budgets. The variant that matches fp32 ships."""
import random, warnings, time, os
warnings.filterwarnings("ignore")
import numpy as np, torch
import onnxruntime as ort
from datasets import load_from_disk
from transformers import AutoTokenizer
from onnxruntime.quantization import quantize_dynamic, QuantType
SP='/private/tmp/claude-501/-Users-peter-Development-iphone-backup-analyzer/815f70cc-17bc-4c6d-b41c-a49f939ad8b8/scratchpad'
random.seed(1)
DIMS=['toxicity','threat','insult','identity_attack','sexual_explicit']
if not os.path.exists(SP+'/civil2_onnx/model_int8pc.onnx'):
    quantize_dynamic(SP+'/civil2_onnx/model.onnx', SP+'/civil2_onnx/model_int8pc.onnx',
                     weight_type=QuantType.QInt8, per_channel=True)
    print("per-channel variant written", flush=True)
ds=load_from_disk(SP+'/datasets/civil_comments')
def collect(split, cap_neg=40000):
    rows=[]; neg=[]
    for r in ds[split]:
        y=[1.0 if r[d]>=0.5 else 0.0 for d in DIMS]
        (rows if max(y)>0 else neg).append((r['text'],y))
    random.shuffle(neg)
    out=rows+neg[:cap_neg]
    return sorted(out,key=lambda x:len(x[0]))
cal=collect('validation'); test=collect('test')
print(f"cal {len(cal)} test {len(test)}", flush=True)
tok=AutoTokenizer.from_pretrained("answerdotai/ModernBERT-base")
def run(model_path, rows, label):
    sess=ort.InferenceSession(model_path, providers=['CPUExecutionProvider'])
    out=[]; ys=[]; t0=time.time()
    for j in range(0,len(rows),64):
        xs=[t for t,_ in rows[j:j+64]]; ys+=[y for _,y in rows[j:j+64]]
        enc=tok(xs,return_tensors='np',padding=True,truncation=True,max_length=192)
        out.append(1/(1+np.exp(-sess.run(None,dict(enc))[0])))
        if j%25600==0: print(f"  {label} {j}/{len(rows)} {time.time()-t0:.0f}s", flush=True)
    return np.concatenate(out), np.array(ys)
grid=sorted(set([x/1000 for x in range(10,1000,5)]+[x/10000 for x in range(9900,10000,5)]))
for name,path in [("fp32", SP+'/civil2_onnx/model.onnx'),
                  ("int8", SP+'/civil2_onnx/model_int8.onnx'),
                  ("int8pc", SP+'/civil2_onnx/model_int8pc.onnx')]:
    pc,yc=run(path,cal,f"{name}-cal"); pt_,yt=run(path,test,f"{name}-test")
    print(f"== {name}", flush=True)
    for j,d in enumerate(DIMS):
        line=[f"{d:16}"]
        for target in [0.01,0.02]:
            best=(None,-1)
            for t in grid:
                pred=pc[:,j]>=t
                fa=int((pred&(yc[:,j]==0)).sum()); nn=int((yc[:,j]==0).sum())
                if fa/nn<=target:
                    c=int((pred&(yc[:,j]>0)).sum())
                    if c>best[1]: best=(t,c)
            if best[0] is None: line.append("--"); continue
            pred=pt_[:,j]>=best[0]
            tp=int((pred&(yt[:,j]>0)).sum()); fn=int(((~pred)&(yt[:,j]>0)).sum())
            fp=int((pred&(yt[:,j]==0)).sum()); tn=int(((~pred)&(yt[:,j]==0)).sum())
            line.append(f"@{target:.0%}: {tp/max(tp+fn,1):.0%} (fa {fp/max(fp+tn,1):.2%})")
        print("  "+"  ".join(line), flush=True)
