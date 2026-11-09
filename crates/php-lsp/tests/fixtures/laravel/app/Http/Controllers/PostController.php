<?php
namespace App\Http\Controllers;

use App\Models\Post;

class PostController {
    public function index() {
        $posts = Post::all();
        return view('posts.index', ['posts' => $posts, 'featured' => Post::find(1)]);
    }

    public function show(int $id) {
        $post = Post::find($id);
        return view('posts.show', compact('post'))->with('related', new Post());
    }
}
