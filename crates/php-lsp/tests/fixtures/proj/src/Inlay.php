<?php
namespace App;

class Mailer {
    public function __construct(private string $host, private int $port = 25) {}
    public function send(string $to, string $subject, ...$extra): void {}
    public static function make(string $host): static { return new static($host, 1); }
}

function greet(string $name, int $times) {}

$host = 'x';
$m = new Mailer('smtp', 587);
$m->send('a@b.c', 'Hi', 1, 2);
$m->send(to: 'x', subject: 'y');
greet($host, 3);
$name = 'n';
greet($name, 2);
str_contains('abc', 'b');
Mailer::make($host);
