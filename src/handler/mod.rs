use std;
use std::fs::{File, write};
use std::io::prelude::*;
use std::str::FromStr;
use std::path::Path;

use chrono::prelude::*;
use reqwest::{Response, header, Client, StatusCode};
use serde_json;
use regex::Regex;
use quick_xml::Reader;
use quick_xml::events::Event;



mod aws;

static S3_FORMAT: &'static str = r#"[sS]3://(?P<bucket>[A-Za-z0-9.]+)(?P<object>[A-Za-z0-9./]*)"#;
static RESPONSE_FORMAT: &'static str = r#""Contents":\["([A-Za-z0-9.]+?)"(.*?)\]"#;


pub enum AuthType{
    AWS4,
    AWS2,
}
pub enum Format{
    JSON,
    XML
}


pub struct Handler<'a>{
    pub host: &'a str,
    pub access_key: &'a str,
    pub secrete_key: &'a str,
    pub auth_type:AuthType,
    pub format: Format
}


pub trait S3 {
    fn la(&self) -> Response;
}

fn print_response(res: &mut Response) -> Vec<u8>{
    let mut body = Vec::new();
    let _ =res.read_to_end(&mut body);
    if res.status() == StatusCode::Ok || res.status() == StatusCode::NoContent {
        info!("Status: {}", res.status());
        info!("Headers:\n{}", res.headers());
        info!("Body:\n{}\n\n", std::str::from_utf8(&body).expect("Body can not decode as UTF8"));
    } else {
        println!("Status: {}", res.status());
        println!("Headers:\n{}", res.headers());
        println!("Body:\n{}\n\n", std::str::from_utf8(&body).expect("Body can not decode as UTF8"));
    }
    body 
}

impl<'a> Handler<'a>  {
    fn aws_v2_request(&self, method: &str, uri: &str, qs: &Vec<(&str, &str)>, payload: &Vec<u8>) -> Result<Vec<u8>, &'static str>{

        let utc: DateTime<Utc> = Utc::now();   
        header! { (Date, "date") => [String] }
        let mut headers = header::Headers::new();
        let time_str = utc.to_rfc2822();
        headers.set(Date(time_str.clone()));

        // NOTE: ceph has bug using x-amz-date
        let mut signed_headers = vec![
            ("Date", time_str.as_str())
        ];

        let mut query_strings = vec![];
        match self.format {
            Format::JSON => {query_strings.push(("format", "json"))}
            _ => {}
        }

        query_strings.extend(qs.iter().cloned());

        let mut query = String::from_str("http://").unwrap();
        query.push_str(self.host);
        query.push_str(uri);
        query.push('?');
        query.push_str(&aws::canonical_query_string(& mut query_strings));
        let signature = aws::aws_s3_v2_sign(
            self.secrete_key, 
            &aws::aws_s3_v2_get_string_to_signed(method, uri, &mut signed_headers, payload)
        );
        let mut authorize_string = String::from_str("AWS ").unwrap();
        authorize_string.push_str(self.access_key);
        authorize_string.push(':');
        authorize_string.push_str(&signature);
        headers.set(header::Authorization(authorize_string));

        // get a client builder
        let client = Client::builder()
            .default_headers(headers)
            .build().unwrap();

        let mut action;
        match method {
            "GET" => {action = client.get(query.as_str());},
            "PUT" => {action = client.put(query.as_str());},
            "DELETE" => {action = client.delete(query.as_str());},
            _ => {
                error!("unspport HTTP verb");
                action = client.get(query.as_str());
            }
        }
        match action.send(){
            Ok(mut res) => Ok(print_response(&mut res)),
            Err(_) => Err("Reqwest Error") //XXX
        }
    }
    fn aws_v4_request(&self, method: &str, uri: &str, qs: &Vec<(&str, &str)>, payload: Vec<u8>) -> Result<Vec<u8>, &'static str>{

        let utc: DateTime<Utc> = Utc::now();   
        header! { (XAMZDate, "x-amz-date") => [String] }
        let mut headers = header::Headers::new();
        let time_str = utc.format("%Y%m%dT%H%M%SZ").to_string();
        headers.set(XAMZDate(time_str.clone()));

        let mut signed_headers = vec![
            ("X-AMZ-Date", time_str.as_str()),
            ("Host",self.host)
        ];

        let mut query_strings = vec![];
        match self.format {
            Format::JSON => {query_strings.push(("format", "json"))}
            _ => {}
        }
        query_strings.extend(qs.iter().cloned());

        let mut query = String::from_str("http://").unwrap();
        query.push_str(self.host);
        query.push_str(uri);
        query.push('?');
        query.push_str(&aws::canonical_query_string(& mut query_strings));
        let signature = 
            aws::aws_v4_sign(self.secrete_key, 
                             aws::aws_v4_get_string_to_signed(
                                  method,
                                  uri,
                                  &mut query_strings,
                                  &mut signed_headers,
                                  &payload,
                                  utc.format("%Y%m%dT%H%M%SZ").to_string()).as_str(),
                              utc.format("%Y%m%d").to_string());
        let mut authorize_string = String::from_str("AWS4-HMAC-SHA256 Credential=").unwrap();
        authorize_string.push_str(self.access_key);
        authorize_string.push('/');
        authorize_string.push_str(&format!("{}/us-east-1/iam/aws4_request, SignedHeaders={}, Signature={}",
                                           utc.format("%Y%m%d").to_string(),
                                           aws::signed_headers(&mut signed_headers), signature));
        headers.set(header::Authorization(authorize_string));

        // get a client builder
        let client = Client::builder()
            .default_headers(headers)
            .build().unwrap();

        let mut action;
        match method {
            "GET" => {action = client.get(query.as_str());},
            "PUT" => {action = client.put(query.as_str()); action.body(payload);},
            "DELETE" => {action = client.delete(query.as_str());},
            _ => {
                error!("unspport HTTP verb");
                action = client.get(query.as_str());
            }
        }
        match action.send(){
            Ok(mut res) => Ok(print_response(&mut res)),
            Err(_) => Err("Reqwest Error") //XXX
        }
    }
    pub fn la(&self) -> Result<(), &'static str> {
        let re = Regex::new(RESPONSE_FORMAT).unwrap();
        let mut res: String;
        match self.auth_type {
            AuthType::AWS4 => { res = std::str::from_utf8(&try!(self.aws_v4_request("GET", "/", &Vec::new(), Vec::new()))).unwrap_or("").to_string();},
            AuthType::AWS2 => { res = std::str::from_utf8(&try!(self.aws_v2_request("GET", "/", &Vec::new(), &Vec::new()))).unwrap_or("").to_string();}
        }
        let result:serde_json::Value;
        let mut buckets = Vec::new();
        match self.format {
            Format::JSON => {
                result = serde_json::from_str(&res).unwrap();
                for bucket_list in  result[1].as_array(){
                    for bucket in bucket_list{
                        buckets.push(bucket["Name"].as_str().unwrap().to_string());
                    }
                }
            },
            Format::XML => {
                let mut reader = Reader::from_str(&res);
                let mut in_name_tag = false;
                let mut buf = Vec::new();

                loop {
                    match reader.read_event(&mut buf) {
                        Ok(Event::Start(ref e)) => {
                            if e.name() == b"Name" { in_name_tag = true; }
                        },
                        Ok(Event::End(ref e)) => {
                            if e.name() == b"Name" { in_name_tag = false; }
                        },
                        Ok(Event::Text(e)) => {
                            if in_name_tag { buckets.push(e.unescape_and_decode(&reader).unwrap()); }
                        },
                        Ok(Event::Eof) => break, 
                        Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
                        _ => (), 
                    }
                    buf.clear();
                }
            }
        }
        for bucket in buckets{
            let bucket_prefix = format!("S3://{}", bucket.as_str());
            match self.auth_type {
                AuthType::AWS4 => { 
                    res = std::str::from_utf8(&try!(self.aws_v4_request("GET", &format!("/{}", bucket.as_str()), &Vec::new(), Vec::new()))).unwrap_or("").to_string();
                    match self.format {
                        Format::JSON => {
                            for cap in re.captures_iter(&res) {
                                println!("{}/{}", bucket_prefix, &cap[1]);
                            }
                        },
                        Format::XML => {
                            let mut reader = Reader::from_str(&res);
                            let mut in_key_tag = false;
                            let mut buf = Vec::new();

                            loop {
                                match reader.read_event(&mut buf) {
                                    Ok(Event::Start(ref e)) => { if e.name() == b"Key" { in_key_tag = true; } },
                                    Ok(Event::End(ref e)) => { if e.name() == b"Key" { in_key_tag = false; } },
                                    Ok(Event::Text(e)) => { if in_key_tag { println!("{}/{}", bucket_prefix, e.unescape_and_decode(&reader).unwrap()); }
                                    },
                                    Ok(Event::Eof) => break, 
                                    Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
                                    _ => (), 
                                }
                                buf.clear();
                            }
                        }
                    }
                },
                AuthType::AWS2 => { 
                    res = std::str::from_utf8(&try!(self.aws_v2_request("GET", &format!("/{}", bucket.as_str()), &Vec::new(), &Vec::new()))).unwrap_or("").to_string();
                    match self.format {
                        Format::JSON => {
                            for cap in re.captures_iter(&res) {
                                println!("{}/{}", bucket_prefix, &cap[1]);
                            }
                        },
                        Format::XML => {
                            let mut reader = Reader::from_str(&res);
                            let mut in_key_tag = false;
                            let mut buf = Vec::new();

                            loop {
                                match reader.read_event(&mut buf) {
                                    Ok(Event::Start(ref e)) => { if e.name() == b"Key" { in_key_tag = true; } },
                                    Ok(Event::End(ref e)) => { if e.name() == b"Key" { in_key_tag = false; } },
                                    Ok(Event::Text(e)) => { if in_key_tag { println!("{}/{}", bucket_prefix, e.unescape_and_decode(&reader).unwrap()); }
                                    },
                                    Ok(Event::Eof) => break, 
                                    Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
                                    _ => (), 
                                }
                                buf.clear();
                            }
                        }
                    }
                }
            }
        };
        Ok(())
    }

    pub fn ls(&self, bucket:Option<&str>)  -> Result<(), &'static str> {
        let res: String;
        match bucket {
            Some(b) => {
                let mut uri:String;
                let mut re = Regex::new(S3_FORMAT).unwrap();
                if b.starts_with("s3://") || b.starts_with("S3://") {
                    let caps = re.captures(b).expect("S3 object format error.");
                    uri = format!("/{}", &caps["bucket"]);
                } else {
                    uri = format!("/{}", b);
                }
                re = Regex::new(RESPONSE_FORMAT).unwrap();
                match self.auth_type {
                    AuthType::AWS4 => {res = std::str::from_utf8(&try!(self.aws_v4_request("GET", &uri, &Vec::new(), Vec::new()))).unwrap_or("").to_string();},
                    AuthType::AWS2 => {res = std::str::from_utf8(&try!(self.aws_v2_request("GET", &uri, &Vec::new(), &Vec::new()))).unwrap_or("").to_string();}
                }
                for cap in re.captures_iter(&res) {
                    println!("s3:/{}/{}", uri, &cap[1]);
                }
            },
            None => {
                match self.auth_type {
                    AuthType::AWS4 => {res = std::str::from_utf8(&try!(self.aws_v4_request("GET", "/", &Vec::new(), Vec::new()))).unwrap_or("").to_string();},
                    AuthType::AWS2 => {res = std::str::from_utf8(&try!(self.aws_v2_request("GET", "/", &Vec::new(), &Vec::new()))).unwrap_or("").to_string();}
                }
                match self.format {
                    Format::JSON => {
                        let result:serde_json::Value = serde_json::from_str(&res).unwrap();
                        for bucket_list in  result[1].as_array(){
                            for bucket in bucket_list{
                                println!("S3://{} ", bucket["Name"].as_str().unwrap());
                            }
                        }
                    },
                    Format::XML => {
                        let mut reader = Reader::from_str(&res);
                        let mut in_name_tag = false;
                        let mut buf = Vec::new();

                        loop {
                            match reader.read_event(&mut buf) {
                                Ok(Event::Start(ref e)) => {
                                    if e.name() == b"Name" { in_name_tag = true }
                                },
                                Ok(Event::End(ref e)) => {
                                    if e.name() == b"Name" { in_name_tag = false }
                                },
                                Ok(Event::Text(e)) => {
                                    if in_name_tag { println!("S3://{} ", e.unescape_and_decode(&reader).unwrap()) }
                                },
                                Ok(Event::Eof) => break, 
                                Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
                                _ => (), 
                            }
                            buf.clear();
                        }
                    }
                }
            } 
        };
        Ok(())
    }

    pub fn put(&self, file:&str, dest:&str) -> Result<(), &'static str> {
        if file == "" || dest == "" {
            return Err("please specify the file and the destiney")
        }
        let re = Regex::new(S3_FORMAT).unwrap();
        let caps = match re.captures(dest){
            Some(c) => c,
            None => return Err("S3 object format error.")
        };
        let mut content: Vec<u8>;


        if !Path::new(file).exists() && file == "test"{ 
            content = vec![83, 51, 82, 83, 32, 116, 101, 115, 116, 10];  // S3RS test/n
        } else {
            let mut fin = match File::open(file) {
                Ok(f) => f,
                Err(_) => return Err("input file open error")
            };
            content = Vec::new();
            let _ = fin.read_to_end(&mut content);
        }

        let uri = if &caps["object"] == "" || &caps["object"] == "/" {
            let file_name =  Path::new(file).file_name().unwrap().to_string_lossy();
            format!("/{}/{}", &caps["bucket"], file_name)
        } else {
            format!("/{}{}", &caps["bucket"], &caps["object"])
        };

        match self.auth_type {
            AuthType::AWS4 => {try!(self.aws_v4_request("PUT", &uri, &Vec::new(), content));},
            AuthType::AWS2 => {try!(self.aws_v2_request("PUT", &uri, &Vec::new(), &content));}
        };
        Ok(())
    }

    pub fn get(&self, src:&str, file:Option<&str>) -> Result<(), &'static str> {
        if src == "" { return Err("Please specify the object")}
        let re = Regex::new(S3_FORMAT).unwrap();
        let caps = match re.captures(src) {
            Some(c) => c,
            None => return Err("S3 object format error.")
        };

        let fout = match file {
            Some(fname) => fname,
            None => {
                Path::new(src).file_name().unwrap().to_str().unwrap_or("s3download")
            }
        };

        if &caps["object"] == ""{
            return Err("Please specific the object")
        }

        match self.auth_type {
            AuthType::AWS4 => {
                match write(fout, try!(self.aws_v4_request("GET", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), Vec::new()))){
                    Ok(_) => return Ok(()),
                    Err(_) => return Err("write file error") //XXX
                }
            },
            AuthType::AWS2 => {
                match write(fout, try!(self.aws_v2_request("GET", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), &Vec::new()))){
                    Ok(_) => return Ok(()),
                    Err(_) => return Err("write file error") //XXX
                }
            }
        }
    }

    pub fn cat(&self, src:&str) -> Result<(), &'static str> {
        if src == "" {return Err("please specific the object")}
        let re = Regex::new(S3_FORMAT).unwrap();
        let caps = match re.captures(src) {
            Some(c) => c,
            None => return Err("S3 object format error.")
        };

        if &caps["object"] == ""{
            return Err("Please specific the object")
        }

        match self.auth_type {
            AuthType::AWS4 => {
                match self.aws_v4_request("GET", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), Vec::new()){
                    Ok(b) => { println!("{}", std::str::from_utf8(&b).unwrap_or("")); return Ok(()) },
                    Err(e) => return Err(e) 
                }
            },
            AuthType::AWS2 => {
                match self.aws_v2_request("GET", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), &Vec::new()){
                    Ok(b) => { println!("{}", std::str::from_utf8(&b).unwrap_or("")); return Ok(()) },
                    Err(e) => return Err(e) 
                }
            }
        }
    }

    pub fn del(&self, src:&str) -> Result<(), &'static str> {
        if src == "" {return Err("please specific the object")}
        let re = Regex::new(S3_FORMAT).unwrap();
        let caps = match re.captures(src) {
            Some(c) => c,
            None => return Err("S3 object format error.")
        };

        if &caps["object"] == ""{
            return Err("Please specific the object")
        }

        match self.auth_type {
            AuthType::AWS4 => {try!(self.aws_v4_request("DELETE", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), Vec::new()));},
            AuthType::AWS2 => {try!(self.aws_v2_request("GET", &format!("/{}{}", &caps["bucket"], &caps["object"]), &Vec::new(), &Vec::new()));}
        }
        Ok(())
    }

    pub fn mb(&self, bucket: &str) -> Result<(), &'static str> {
        if bucket == "" {return Err("please specific the bucket name")}
        let mut uri = String::from_str("/").unwrap();
        uri.push_str(bucket);
        match self.auth_type {
            AuthType::AWS4 => {try!(self.aws_v4_request("PUT", &uri, &Vec::new(), Vec::new()));},
            AuthType::AWS2 => {try!(self.aws_v2_request("PUT", &uri, &Vec::new(), &Vec::new()));}
        };
        Ok(())
    }

    pub fn rb(&self, bucket: &str) -> Result<(), &'static str> {
        if bucket == "" {return Err("please specific the bucket name")}
        let mut uri = String::from_str("/").unwrap();
        uri.push_str(bucket);
        match self.auth_type {
            AuthType::AWS4 => {try!(self.aws_v4_request("DELETE", &uri, &Vec::new(), Vec::new()));},
            AuthType::AWS2 => {try!(self.aws_v2_request("DELETE", &uri, &Vec::new(), &Vec::new()));}
        };
        Ok(())
    }

    pub fn url_command(&self, url: &str) -> Result<(), &'static str>  {
        let mut uri = String::new();
        let mut raw_qs = String::new();
        let mut query_strings = Vec::new();
        match url.find('?'){
            Some(idx) =>{
                uri.push_str(&url[..idx]);
                raw_qs.push_str(&String::from_str(&url[idx+1..]).unwrap());
                for q_pair in raw_qs.split('&'){
                    match q_pair.find('='){
                        Some(i)=>{query_strings.push(q_pair.split_at(i))},
                        None => {query_strings.push((&q_pair, ""))}
                    }
                }
            },
            None => {
                uri.push_str(&url);
            }
        }

        match self.auth_type {
            AuthType::AWS4 => {try!(self.aws_v4_request("GET", &uri, &query_strings, Vec::new()));},
            AuthType::AWS2 => {try!(self.aws_v2_request("GET", &uri, &query_strings, &Vec::new()));}
        };
        Ok(())
    }
}
