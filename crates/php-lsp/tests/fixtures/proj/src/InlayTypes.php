<?php
namespace App;

class Post {}
class Author {
    /** @return Post[] */
    public function posts(): array { return []; }
    public static function find(int $id): ?Author { return null; }
    public function first(): Post { return new Post(); }
}

function run(Author $typed, $untyped) {
    $author = Author::find(1);
    $created = new Author();
    $posts = $author->posts();
    foreach ($posts as $post) {}
    /** @var Post $tagged */
    $tagged = $author->first();
    try {} catch (\Exception $e) {}
    $fn = function ($p) {};
    array_map(fn($x) => $x, $posts);
    $copy = $author;
}
