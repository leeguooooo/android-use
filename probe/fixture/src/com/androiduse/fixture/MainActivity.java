package com.androiduse.fixture;
import android.app.Activity;
import android.os.Bundle;
import android.text.InputType;
import android.view.View;
import android.webkit.WebView;
import android.widget.*;
public class MainActivity extends Activity {
 public void onCreate(Bundle state) {
  super.onCreate(state);
  ScrollView scroll=new ScrollView(this); LinearLayout body=new LinearLayout(this);body.setOrientation(1);scroll.addView(body);setContentView(scroll);
  TextView title=new TextView(this);title.setText("android-use synthetic fixture 🦊");body.addView(title);
  EditText normal=new EditText(this);normal.setHint("Normal placeholder 🦊");normal.setContentDescription("Normal input");body.addView(normal);
  EditText multi=new EditText(this);multi.setHint("Multiline placeholder");multi.setInputType(InputType.TYPE_CLASS_TEXT|InputType.TYPE_TEXT_FLAG_MULTI_LINE);body.addView(multi);
  EditText password=new EditText(this);password.setHint("Synthetic password");password.setInputType(InputType.TYPE_CLASS_TEXT|InputType.TYPE_TEXT_VARIATION_PASSWORD);body.addView(password);
  Button counter=new Button(this);counter.setText("Count 0");counter.setOnClickListener(new View.OnClickListener(){int n=0;public void onClick(View v){counter.setText("Count "+(++n));}});body.addView(counter);
  WebView web=new WebView(this);web.loadDataWithBaseURL("https://fixture.invalid/","<html><meta name='viewport' content='width=device-width, initial-scale=1'><body><label for='field'>Offline web input</label><input id='field' placeholder='Web placeholder'></body></html>","text/html","UTF-8",null);body.addView(web,new LinearLayout.LayoutParams(-1,250));
  for(int i=0;i<40;i++){TextView row=new TextView(this);row.setText("Fixture row "+i);row.setTextSize(22);body.addView(row);}
 }
}
