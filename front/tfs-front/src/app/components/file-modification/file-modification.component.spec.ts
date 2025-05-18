import { ComponentFixture, TestBed } from '@angular/core/testing';

import { FileModificationComponent } from './file-modification.component';

describe('FileModificationComponent', () => {
  let component: FileModificationComponent;
  let fixture: ComponentFixture<FileModificationComponent>;

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [FileModificationComponent]
    })
    .compileComponents();

    fixture = TestBed.createComponent(FileModificationComponent);
    component = fixture.componentInstance;
    fixture.detectChanges();
  });

  it('should create', () => {
    expect(component).toBeTruthy();
  });
});
