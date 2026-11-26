#!/usr/bin/env python3
"""Generátor tabuliek SILK (media/src/opus/silk_tabulky.rs) z textu RFC 6716 – vstup: časti §4.2 ako text
(/tmp/rfc/silk1–6.txt, zo zdroja RFC v repozitári xiph/opus doc/draft-ietf-codec-opus.xml, bez prílohy A)."""
import re,sys
T={i:open(f'/tmp/rfc/silk{i}.txt').read() for i in range(1,7)}
def nums(s): return [int(x) for x in re.findall(r'-?\d+',s)]
def pdf_after(txt, anchor, start=0):
    i=txt.index(anchor,start); j=txt.index('{',i); k=txt.index('}',j)
    return nums(txt[j:k]), k
out=[]
def emit(name, typ, vals, per=16):
    body=', '.join(str(v) for v in vals)
    out.append(f"pub const {name}: [{typ}; {len(vals)}] = [{body}];")
def emit2(name, typ, rows):
    n=len(rows[0]); assert all(len(r)==n for r in rows), name
    body=',\n    '.join('['+', '.join(map(str,r))+']' for r in rows)
    out.append(f"pub const {name}: [[{typ}; {n}]; {len(rows)}] = [\n    {body},\n];")
def emitv(name, typ, rows):
    body=',\n    '.join('&['+', '.join(map(str,r))+']' for r in rows)
    out.append(f"pub const {name}: [&[{typ}]; {len(rows)}] = [\n    {body},\n];")
s1=T[1]
v,_=pdf_after(s1,'40 ms ;'); emit('LBRR_40','u16',v)
v,_=pdf_after(s1,'60 ms ;'); emit('LBRR_60','u16',v)
p=s1.index('Stage 1 ;'); v,k=pdf_after(s1,'Stage 1 ;'); assert len(v)==25; emit('STEREO_1','u16',v)
v,_=pdf_after(s1,'Stage 2 ;'); emit('STEREO_2','u16',v)
v,_=pdf_after(s1,'Stage 3 ;'); emit('STEREO_3','u16',v)
i=s1.index('Weight (Q13)'); w=[]
for line in s1[i:].split('\n')[1:17]:
    a=nums(line); w.append(a[1])
emit('STEREO_VAHY_Q13','i32',w)
i=s1.index('### Mid-only Flag'); v,_=pdf_after(s1,'PDF',i); emit('LEN_STRED','u16',v)
v,_=pdf_after(s1,'Inactive ; {'.replace(' {',''),s1.index('### Frame Type')); emit('TYP_NEAKTIVNY','u16',v)
v,_=pdf_after(s1,'Active ;',s1.index('### Frame Type')); emit('TYP_AKTIVNY','u16',v)
g=s1.index('### Subframe Gains')
rows=[]
for a in ['Inactive ;','Unvoiced ;','Voiced ;']:
    v,_=pdf_after(s1,a,g); rows.append(v)
emit2('ZISK_MSB','u16',rows)
v,_=pdf_after(s1,'{32, 32',g-1) ; emit('ZISK_LSB','u16',v)
i=s1.index('delta_gain_index value'); v,_=pdf_after(s1,'PDF',i); assert len(v)==41; emit('ZISK_DELTA','u16',v)
s2=T[2]
rows=[]; p=0
for a in ['NB or MB ; Inactive','NB or MB ; Voiced','WB ; Inactive','WB ; Voiced']:
    v,_=pdf_after(s2,a); assert len(v)==32; rows.append(v)
emit2('NLSF_1','u16',rows)
rows=[]
for c in 'abcdefghijklmnop':
    v,_=pdf_after(s2,f'\n{c} ;'); rows.append(v)
emit2('NLSF_2','u16',rows)
def letter_table(txt, start, ncols, letters):
    lines=txt[start:].split('\n'); res=[]; i=0
    while len(res)<32:
        l=lines[i].strip(); i+=1
        toks=l.replace(';','').split()
        if len(toks)==ncols and all(t in letters for t in toks): res.append(toks)
    return res
i=s2.index('0 1 2 3 4 5 6 7 8 9 ;'); t=letter_table(s2,i+10,10,'abcdefgh'); emit2('NLSF_2_VYBER_NBMB','u8',[[ord(x)-ord('a') for x in r] for r in t])
i=s2.index('0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 ;'); t=letter_table(s2,i+10,16,'ijklmnop'); emit2('NLSF_2_VYBER_WB','u8',[[ord(x)-ord('i')+8 for x in r] for r in t])
i=s2.index('{156, 60'); v,_=pdf_after(s2,'PDF',i-10); emit('NLSF_2_ROZSIRENIE','u16',v)
i=s2.index('Coefficient\nA\nB\nC\nD'); lines=s2[i:].split('\n')[5:20]
A=[];B=[];C=[];D=[]
for l in lines:
    a=nums(l.replace('<c/>',''))
    if len(a)==5: A.append(a[1]);B.append(a[2]);C.append(a[3]);D.append(a[4])
    else: C.append(a[1]);D.append(a[2])
assert len(A)==9 and len(C)==15
emit('NLSF_PRED_A','i32',A);emit('NLSF_PRED_B','i32',B);emit('NLSF_PRED_C','i32',C);emit('NLSF_PRED_D','i32',D)
i=s2.index('0 1 2 3 4 5 6 7 8 ;'); t=letter_table(s2,i+10,9,'AB'); emit2('NLSF_PRED_VYBER_NBMB','u8',[[ord(x)-ord('A') for x in r] for r in t])
i=s2.index('0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 ;'); t=letter_table(s2,i+10,15,'CD'); emit2('NLSF_PRED_VYBER_WB','u8',[[ord(x)-ord('C') for x in r] for r in t])
def num_table(txt,start,ncols,nrows=32):
    lines=txt[start:].split('\n'); res=[]; i=0
    while len(res)<nrows:
        a=nums(lines[i].replace(';','')); i+=1
        if len(a)==ncols: res.append(a)
    return res
i=s2.index(' 0   1   2   3   4   5   6   7   8   9 ;'); emit2('NLSF_CB_NBMB','i32',num_table(s2,i+40,10))
i=s2.index(' 0  1  2  3  4   5   6   7   8   9  10  11  12  13  14  15 ;'); emit2('NLSF_CB_WB','i32',num_table(s2,i+60,16))
i=s2.index('NB and MB\nWB'); lines=s2[i:].split('\n')[2:19]; nb=[];wb=[]
for l in lines:
    a=nums(l.replace('<c/>',''))
    if len(a)==3: nb.append(a[1]); wb.append(a[2])
    else: wb.append(a[1])
assert len(nb)==11 and len(wb)==17
emit('NLSF_MIN_NBMB','i32',nb); emit('NLSF_MIN_WB','i32',wb)
i=s2.index('{13, 22, 29'); v,_=pdf_after(s2,'PDF',i-10); emit('NLSF_INTERP','u16',v)
s3=T[3]
i=s3.index('NB and MB\nWB'); lines=s3[i:].split('\n')[2:18]; nb=[];wb=[]
for l in lines:
    a=nums(l.replace('<c/>',''))
    if len(a)==3: nb.append(a[1]); wb.append(a[2])
    else: wb.append(a[1])
assert len(nb)==10 and len(wb)==16
emit('PORADIE_NBMB','usize',nb); emit('PORADIE_WB','usize',wb)
i=s3.index('+0\n+1\n+2\n+3'); j=s3.index('Given the list of cosine'); c=[]
for l in s3[i+12:j].split('\n'):
    a=nums(l.replace('<c/>',''))
    if len(a)==4 or (len(a)==1 and a[0]==-4096 and len(c)==128): c+=a
assert len(c)==129, len(c)
emit('COS_Q12','i32',c)
s4=T[4]
v,_=pdf_after(s4,'PDF'); assert len(v)==32; emit('LAG_VYSOKA','u16',v)
rows=[]
for a in ['NB ;','MB ;','WB ;']:
    v,_=pdf_after(s4,a); rows.append(v)
emitv('LAG_NIZKA','u16',rows)
i=s4.index('{46,  2'); v,_=pdf_after(s4,'PDF',i-10); assert len(v)==21; emit('LAG_DELTA','u16',v)
rows=[]
for a in ['NB ;       10 ms','NB ;       20 ms','MB or WB ; 10 ms','MB or WB ; 20 ms']:
    v,_=pdf_after(s4,a); rows.append(v)
emitv('KONTURA','u16',rows)
i=s4.index('Index\nSubframe Offsets')
def offs(start,n,k):
    lines=s4[start:].split('\n'); res=[]; i=0
    while len(res)<n:
        a=nums(lines[i].replace(';','')); i+=1
        if len(a)==k+1: res.append(a[1:])
    return res
starts=[m.start() for m in re.finditer('Index\nSubframe Offsets',s4)]
emit2('KONTURA_NB_10','i32',offs(starts[0],3,2))
emit2('KONTURA_NB_20','i32',offs(starts[1],11,4))
emit2('KONTURA_10','i32',offs(starts[2],12,2))
emit2('KONTURA_20','i32',offs(starts[3],34,4))
i=s4.index('{77, 80, 99}'); emit('PERIODICITA','u16',[77,80,99])
rows=[]
for a in ['0 ;  8 ;','1 ; 16 ;','2 ; 32 ;']:
    v,_=pdf_after(s4,a); rows.append(v)
emitv('LTP_FILTER','u16',rows)
st=[m.start() for m in re.finditer('Index\nFilter Taps \\(Q7\\)',s4)]
for nm,s0,n in [('LTP_TAPY_0',st[0],8),('LTP_TAPY_1',st[1],16),('LTP_TAPY_2',st[2],32)]:
    lines=s4[s0:].split('\n'); res=[]; i=2
    while len(res)<n:
        a=nums(lines[i]); i+=1
        if len(a)==5: res.append(a)
    emit2(nm,'i32',res)
i=s4.index('### LTP Scaling'); v,_=pdf_after(s4,'PDF',i); emit('LTP_SKALA','u16',v)
i=s4.index('### Linear Congruential'); v,_=pdf_after(s4,'PDF',i); emit('SEMIENKO','u16',v)
s5=T[5]
rows=[]
for a in ['Inactive or Unvoiced ;','Voiced ;']:
    v,_=pdf_after(s5,a,s5.index('### Rate Level')); rows.append(v)
emit2('UROVEN','u16',rows)
i=s5.index('### Pulses Per Shell Block'); rows=[]
for r in range(11):
    v,_=pdf_after(s5,f'\n{r} ;',i); assert len(v)==18, (r,len(v)); rows.append(v)
emit2('PULZY','u16',rows)
st=[m.start() for m in re.finditer('Pulse Count\nPDF',s5)]
for nm,s0 in zip(['SHELL_16','SHELL_8','SHELL_4','SHELL_2'],st):
    rows=[]
    for c in range(1,17):
        v,_=pdf_after(s5,f'\n{c:2d} ;',s0); assert len(v)==c+1; rows.append(v)
    emitv(nm,'u16',rows)
i=s5.index('### LSB Decoding'); v,_=pdf_after(s5,'PDF',i); emit('LSB','u16',v)
i=s5.index('Signal Type\nQuantization Offset Type\nPulse Count'); rows=[]
for l in s5[i:].split('\n'):
    m=re.search(r'\{(\d+), (\d+)\}/256',l)
    if m: rows.append([int(m.group(1)),int(m.group(2))])
    if len(rows)==42: break
emit2('ZNAMIENKO','u16',rows)
print('\n'.join(out))
