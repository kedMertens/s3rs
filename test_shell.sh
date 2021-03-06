#!/usr/bin/expect -f

# usage ./test_command.sh <number_of_your_profile> <bucket_name>

set item [lindex $argv 0]
set bucket [lindex $argv 1]
set prompt "s3rs.*>";
set timeout 120

spawn dd if=/dev/zero bs=1024 count=7000 of=/tmp/7M
spawn cargo run

expect "Selection:"
send $item\r

expect -re $prompt
send ls\r

expect -re $prompt
send "put test s3://$bucket\r"

expect -re $prompt
send "ls s3://$bucket\r"

expect -re $prompt
send "ls /$bucket\r"

expect -re $prompt
send "ls $bucket\r"

expect -re $prompt
send "la\r"

expect -re $prompt
send "ll s3://$bucket/te\r"

expect -re $prompt
send "cat s3://$bucket/test\r"

expect -re $prompt
send "tag add s3://$bucket/test a=1 b=2\r"

expect -re $prompt
send "tag ls s3://$bucket/test\r"

expect -re $prompt
send "tag del s3://$bucket/test\r"

expect -re $prompt
send "tag list s3://$bucket/test\r"

expect -re $prompt
send "rm s3://$bucket/test\r"

expect -re $prompt
send "ll $bucket\r"

expect -re $prompt
send "info $bucket\r"

expect -re $prompt
send "logout\n"

expect "Selection:"
send $item\r

expect -re $prompt
send "log info\r"

expect -re $prompt
send "put /tmp/7M s3://$bucket\r"

expect -re $prompt
send "get s3://$bucket/7M /tmp/7\r"

expect -re $prompt
send "exit\r"

expect "cya~"
spawn md5sum /tmp/7M /tmp/7

interact

