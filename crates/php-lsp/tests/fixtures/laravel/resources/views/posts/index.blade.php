@extends('layouts.app')

@section('content')
    <h1>Gönderiler — {{ count($posts) }}</h1>
    @foreach ($posts as $post)
        <p>Başlık: {{ $post->title }} · {{ $post->author()->name }}</p>
    @endforeach
    {{ $featured->title }}
    {{ $featured->nope() }}
    @include('users.show')
@endsection
