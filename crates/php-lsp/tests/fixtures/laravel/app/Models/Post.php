<?php
namespace App\Models;

class Author {
    public string $name = '';
}

class Post {
    public string $title = '';

    /** @return Post[] */
    public static function all(): array { return []; }

    public static function find(int $id): ?Post { return null; }

    public function author(): Author { return new Author(); }
}
