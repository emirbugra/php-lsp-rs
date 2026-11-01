<?php
namespace App;

interface Shape { public function area(): float; }
abstract class Base implements Shape { abstract public function area(): float; public function name(): string { return 'b'; } }
class Circle extends Base { public function area(): float { return 3.14; } }
class Square extends Base { public function area(): float { return 1.0; } public function name(): string { return 's'; } }
class Cube extends Square {}

function use_it(Shape $s, Base $b) {
    $s->/*1*/area();
    $b->/*2*/name();
}
