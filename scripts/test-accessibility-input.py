#!/usr/bin/env python3
"""Run on an installed synthetic fixture via the existing private daemon config.
Build/install probe/build/fixture.apk first. Does not use ADB during input checks.
"""
import pathlib,json,urllib.request,urllib.error,time,argparse
parser=argparse.ArgumentParser()
parser.add_argument('--output',type=pathlib.Path,default=pathlib.Path('docs/measurements/accessibility/input-results.json'))
args=parser.parse_args()
args.output.parent.mkdir(parents=True,exist_ok=True)
config=json.loads((pathlib.Path.home()/'.android-use/connection.json').read_text())
base=config['url'];owner='accessibility-input-qa'
def call(path,data=None):
 req=urllib.request.Request(base+path,data=None if data is None else json.dumps(data,ensure_ascii=False).encode(),headers={'Authorization':'Bearer '+config['token'],'X-Phone-Owner':owner,'Content-Type':'application/json'})
 start=time.perf_counter()
 try:
  with urllib.request.urlopen(req,timeout=30) as response: value=json.load(response)
 except urllib.error.HTTPError as error: value=json.load(error)
 return value,round((time.perf_counter()-start)*1000,2)
status,_=call('/agent/status')
assert status.get('backend')=='accessibility' and status.get('drivable'),status

records=[]
call('/agent/actions',{'steps':[{'type':'launch_app','app':'com.androiduse.fixture'},{'type':'wait_for','expect':{'app':'com.androiduse.fixture'},'timeout_ms':5000}],'observe':False})
def read():
 v,ms=call('/agent/elements');assert v.get('ok'),v;assert v['app']=='com.androiduse.fixture',v.get('app');return v

def act(action):
 v,ms=call('/agent/actions',{'steps':[action],'observe':False});return v,ms

def focus(label=None,password=False,web=False):
 # A refused stale target is safe to resolve again from a new observation.
 # An acknowledged/unknown tap is never replayed.
 for attempt in range(6):
  time.sleep(.25)
  v=read()
  rows=[r for r in v['elements'] if r['kind']=='TextField' and r['visible'] and (r.get('password') if password else not r.get('password')) and (r['label']==label if label else True)]
  if web:rows=[r for r in rows if not r['label']]
  assert len(rows)==1,(label,len(rows))
  result,ms=call('/agent/actions',{'steps':[{'type':'tap','element':rows[0]['index'],'snapshot':v['snapshot']}],'observe':True})
  if result.get('outcome')=='not_sent' and result.get('error') in ('stale_snapshot','invalid_target'):continue
  assert result.get('ok'),result
  return rows[0]['index']
 raise AssertionError('No stable actionable input target')

def check_type(name,text,clear,expected=None,password=False):
 v,ms=act({'type':'type','text':text,'clear':clear});s=read();focused=[r for r in s['elements'] if r.get('focused') and r['kind']=='TextField']
 passed=v.get('ok')==True and len(focused)==1 and (focused[0]['value']==expected if expected is not None else focused[0]['value']=='' and focused[0]['label']=='<password>')
 records.append({'case':name,'passed':passed,'ms':ms,'outcome':v.get('outcome'),'error':v.get('error')});args.output.write_text(json.dumps(records,indent=2));print(json.dumps(records[-1]),flush=True)
 if not passed:raise AssertionError(name)
v,ms=call('/agent/actions',{'steps':[{'type':'wait_for','expect':{'app':'com.androiduse.fixture'},'timeout_ms':5000}],'observe':False});assert v.get('ok'),v
focus('Normal placeholder 🦊')
check_type('empty_hint_append_unicode','你好 日本語 🦊',False,'你好 日本語 🦊')
check_type('append_at_cursor',' + 👩‍💻',False,'你好 日本語 🦊 + 👩‍💻')
check_type('replace_unicode','Replacement 🦊',True,'Replacement 🦊')
check_type('clear_to_hint','',True,'')
# Hide keyboard before finding the next field.
act({'type':'back'})
focus('Multiline placeholder')
check_type('multiline','first\n第二行 🦊',True,'first\n第二行 🦊')
act({'type':'back'})
focus(password=True)
check_type('synthetic_password_redacted','Fixture-only-123!',True,None,True)
v,ms=act({'type':'type','text':'x','clear':False});passed=v.get('ok')==False and v.get('outcome')=='not_sent';records.append({'case':'password_append_refused','passed':passed,'ms':ms,'outcome':v.get('outcome'),'error':v.get('error')});args.output.write_text(json.dumps(records,indent=2));print(json.dumps(records[-1]),flush=True);assert passed
act({'type':'back'})
focus(web=True)
check_type('offline_webview_unicode','Web 日本語 🦊',True,'Web 日本語 🦊')
args.output.write_text(json.dumps(records,indent=2))
call('/agent/release',{})
