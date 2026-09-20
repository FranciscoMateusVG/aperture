"""Generate design-native SVG references, NOT screenshots or runtime evidence."""
from pathlib import Path
from html import escape
import hashlib, json
P=Path(__file__).parent
BG='#0f0f1a'; CARD='#1a1a2e'; PANEL='#141428'; TXT='#e0e0e0'; MUT='#a6a6c0'; BORDER='#666684'; AMBER='#f39c12'
files=[]
for W,H in [(1280,800),(1024,768)]:
 for screen,state in [('library','default'),('wizard','default'),('grouped','mixed'),('archive','blocked')]+[('replace',s) for s in ['checkpoint_pending','stopping','revoking','remote_uncertain','ready','blocked']]:
  out=[f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" aria-label="Aperture V4 {screen} {state} design reference">',f'<rect width="{W}" height="{H}" fill="{BG}"/>']
  def rect(x,y,w,h,fill=CARD,stroke='#2a2a4a',r=6): out.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}" stroke="{stroke}"/>')
  def t(x,y,text,size=14,c=TXT,weight=400): out.append(f'<text x="{x}" y="{y}" fill="{c}" font-family="-apple-system,BlinkMacSystemFont,sans-serif" font-size="{size}" font-weight="{weight}">{escape(text)}</text>')
  def btn(x,y,w,text,primary=False,disabled=False):
   rect(x,y,w,44,AMBER if primary and not disabled else PANEL,BORDER)
   t(x+14,y+28,text,14,BG if primary and not disabled else MUT if disabled else TXT,600)
  def field(x,y,w,label,value):
   t(x,y,label,12,MUT); rect(x,y+10,w,44,PANEL,BORDER);t(x+12,y+38,value)
  rect(0,0,W,64,'#0c0c18',r=0); t(24,39,'APERTURE',16,AMBER,700)
  t(205,39,'Sessions',14,TXT if screen=='grouped' else MUT); t(308,39,'Presets',14,TXT if screen=='library' else MUT)
  t(W-244,39,'DESIGN • synthetic fixture',12,MUT)
  if screen=='library':
   t(32,117,'A team starts with a clear mission.',28,TXT,600)
   t(32,149,'Reusable presets. Independent team snapshots. Your standing roster stays intact.',14,MUT)
   btn(W-211,174,179,'New blank preset')
   gap=20; cw=(W-64-gap*2)/3
   for i,(name,desc,roles) in enumerate([('Fullstack','Build and verify a product slice.',['Backend · lead · Codex','Frontend · Claude','QA · Claude']),('Review','Independent, bounded review.',['Reviewer · lead · Codex','QA · Claude']),('Custom preset','Define your own team shape.',['Choose roles and a lead','Set model and reasoning'])]):
    x=32+i*(cw+gap);rect(x,242,cw,374)
    t(x+20,279,name,20,TXT,600);t(x+20,307,desc,12,MUT)
    for j,role in enumerate(roles):t(x+20,352+j*30,role,14)
    t(x+20,458,'Models are configuration, not observation.',12,MUT)
    btn(x+20,478,cw-40,'New team from preset',True)
    btn(x+20,540,(cw-48)/2,'Edit');btn(x+28+(cw-48)/2,540,(cw-48)/2,'Duplicate')
   t(32,668,'Edits affect future teams only. Existing teams retain their original snapshot.',14,MUT)
  elif screen=='wizard':
   t(32,110,'New team',28,TXT,600);t(32,138,'From Fullstack · creating a team requests approval; it does not start workers.',14,MUT)
   field(32,177,(W-84)/2,'Team name','fitt-relaunch');field((W+20)/2,177,(W-84)/2,'Project','project:incluir')
   field(32,258,W-64,'Mission','Rebuild the booking experience within the approved scope.')
   field(32,339,W-64,'Acceptance','Review passed; agreed journey verified; evidence attached.')
   t(32,419,'Seats',20,TXT,600); t(250,419,'Exactly one lead. Names are derived, never typed independently.',12,MUT)
   for j,(role,harness,model,reason) in enumerate([('backend','Codex','gpt-6-astra','high'),('frontend','Claude','opus','—'),('qa','Claude','sonnet','—')]):
    y=438+j*50;rect(32,y,W-64,44,PANEL)
    t(46,y+28,('● ' if j==0 else '○ ')+f'fitt-relaunch-{role}',14)
    t(W*.43,y+28,role);t(W*.57,y+28,harness);t(W*.68,y+28,model);t(W*.87,y+28,reason)
   btn(32,596,136,'+ Add seat');t(188,623,'Configured fallbacks: gpt-5.6-sol · gpt-5.6-terra',14,MUT)
   btn(W-304,H-88,120,'Cancel');btn(W-168,H-88,136,'Create team',True)
  elif screen=='grouped':
   t(32,108,'Sessions',28,TXT,600);btn(W-179,80,147,'Browse presets')
   t(32,151,'COORDINATION',12,MUT,600)
   for i,name in enumerate(['GLaDOS','Wheatley','Peppy']):
    x=32+i*(W-64)/3;rect(x,169,(W-88)/3,66);t(x+16,195,name,16,TXT,600);t(x+16,218,'Existing roster · state from launcher',12,MUT)
   t(32,277,'Standing specialists',18,TXT,600); t(280,277,'Preserved during migration',12,MUT)
   rect(32,297,W-64,54);t(48,330,'Vance    Rex    Izzy    Scout    Cipher — existing controls unchanged',14,MUT)
   t(32,402,'fitt-relaunch',22,TXT,600);t(252,402,'ACTIVE · lead: backend · epic: fixture-001',12,MUT)
   btn(W-179,371,147,'Archive…');t(32,433,'Mission: rebuild the booking experience',14,MUT)
   rect(32,452,W-64,171);t(48,484,'fitt-relaunch-backend · LEAD',16,TXT,600);t(W-228,484,'g2 · rate-limited',14,AMBER)
   t(48,516,'Configured: Codex / gpt-6-astra / high',14,MUT);t(W/2,516,'Observed: unknown — not verified',14,AMBER)
   t(48,548,'Checkpoint: stale · Context: unavailable',14,MUT)
   for i,(label,bw) in enumerate([('Open',90),('Checkpoint now',165),('Replace worker…',170),('Stop',90)]):
    x=[48,150,327,509][i];btn(x,563,bw,label)
   rect(32,647,W-64,58);t(48,672,'next-slice · PENDING',14,AMBER,600);t(48,694,'Waiting for epic approval. No workers visible or started.',12,MUT)
  else:
   dw=min(W-96,850);x=(W-dw)/2;y=100;rect(x,y,dw,H-154,CARD,BORDER,12)
   t(x+28,y+43,'Archive team' if screen=='archive' else 'Replace worker',24,TXT,600)
   t(x+28,y+73,'fitt-relaunch' if screen=='archive' else 'fitt-relaunch-backend · expected generation 2',14,MUT)
   if screen=='archive':
    t(x+28,y+115,'Nothing is deleted. The backend validates every condition.',14,MUT)
    rows=[('Complete','Reconciliation record exists'),('Blocked','fixture-002: completed without evidence'),('Blocked','Required review is missing'),('Blocked','Transfer has no receiving-owner acceptance'),('Unknown','Live processes and remote effects not verified')]
    for j,(status,desc) in enumerate(rows):
     yy=y+145+j*60;rect(x+28,yy,dw-56,52,PANEL);t(x+42,yy+23,status,12,AMBER if status!='Complete' else '#2ecc71',600);t(x+42,yy+43,desc,14)
    btn(x+28,H-130,174,'Check readiness');btn(x+dw-258,H-130,100,'Cancel');btn(x+dw-146,H-130,118,'Archive',disabled=True)
   else:
    field(x+28,y+115,150,'Harness','Codex');field(x+194,y+115,240,'Replacement model','gpt-5.6-sol');field(x+450,y+115,140,'Reasoning','high')
    t(x+28,y+207,'Checkpoint: stale    •    Observed model: unverified',14,MUT)
    labels={'checkpoint_pending':'Requesting checkpoint…','stopping':'Stopping the owned process tree…','revoking':'Revoking hub/message authority…','remote_uncertain':'Blocked: remote effects are uncertain','ready':'Ready permit received; start revalidates it','blocked':'Blocked: owned process stop not verified'}
    t(x+28,y+248,labels[state],18,AMBER,600)
    rows=['Checkpoint result recorded','Owned processes stopped','Hub/message authority revoked','Remote effects reconciled','Worktree inventoried without modification']
    done={'checkpoint_pending':0,'stopping':1,'revoking':2,'remote_uncertain':3,'ready':5,'blocked':1}[state]
    for j,label in enumerate(rows):t(x+28,y+289+j*32,('✓ ' if j<done else '○ ')+label,14,'#2ecc71' if j<done else MUT)
    t(x+28,H-198,'Checks are backend evidence, not operator-ticked checkboxes.',12,MUT)
    t(x+28,H-174,'Closing this dialog does not undo a stop or revoke already requested.',12,MUT)
    btn(x+28,H-130,205,'Prepare / stop / verify',disabled=state in ['checkpoint_pending','stopping','revoking'])
    btn(x+dw-303,H-130,100,'Close');btn(x+dw-191,H-130,163,'Start replacement',primary=state=='ready',disabled=state!='ready')
  t(24,H-16,'Design reference only · no command invoked · not an installed-app capture',11,MUT)
  out.append('</svg>')
  name=f'{screen}-{state}-{W}x{H}.svg';data='\n'.join(out)+'\n';(P/name).write_text(data)
  files.append({'file':name,'sha256':hashlib.sha256(data.encode()).hexdigest(),'viewport':[W,H],'format':'design-native SVG'})
manifest={'status':'awaiting-root-visual-decision','approver':None,'sourceBase':'2703dc47d0f41f2c0e0dc417ed7c14d8df4a393a','sources':['src/style.css','spec v2.7 §3 ASCII mockups'],'theme':'existing dark navy/amber (proposed; spec light discrepancy raised)','capture':'NOT_CAPTURED: authored vectors, logical geometry only; no WKWebView/browser/runtime/pixel-match claim','devicePixelRatio':None,'font':'-apple-system, BlinkMacSystemFont, sans-serif','animations':'none','references':files}
(P/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
